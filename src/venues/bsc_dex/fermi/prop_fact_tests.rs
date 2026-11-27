//! Проверка покупки WETH через PropAMM по архивным квитанциям Kaliber и Taurus.
//! Исходная подпись задаёт минимальное число атомов, фактическую выплату даёт расчёт по событиям.
use super::{Proof, fill, send};
use crate::core::types::{Asset, ChainId, Side, TradeRequest, VenueId};
use crate::venues::bsc_dex::{fermi::recovery, num};
use alloy_primitives::{Address, U256, keccak256};
use rust_decimal::Decimal;

pub(super) fn checked_buy(
    proof: &Proof, receipt: &send::receipt::Receipt, router: Address, paid: U256, received: U256,
) {
    let raw = fill(proof, receipt, router).unwrap();
    let minimum = proof.prop.as_ref().unwrap().minimum;
    let mut req = TradeRequest {
        venue: VenueId("eth_dex".into()),
        asset: Asset { ticker: "WETH".into(), contract: Some(proof.output), chain: Some(ChainId("ethereum".into())) },
        deal_id: proof.leg.to_string(), side: Side::Buy, qty: num::to_decimal(minimum, 18).unwrap(),
        worst_price: num::to_decimal(paid, 6).unwrap() / num::to_decimal(minimum, 18).unwrap(),
        route: None, exit_position: false, remeasure: None,
    };
    let (qty, gross) = recovery::amounts(proof, &req, (18, 6), raw).unwrap();
    assert_eq!(qty, num::to_decimal(received, 18).unwrap());
    assert_eq!(gross, num::to_decimal(paid, 6).unwrap());
    assert!(qty > req.qty);
    // Известные деньги в квитанции сохраняются, но к заявке с другим пределом их привязать нельзя.
    req.qty += req.qty;
    assert!(recovery::amounts(proof, &req, (18, 6), raw).is_none());
    assert_eq!(fill(proof, receipt, router), Some((paid, received)));

    let incoming = receipt.logs.iter().find(|log|
        log.address() == proof.output && log.topic0() == Some(&keccak256("Transfer(address,address,uint256)"))
            && log.topics().get(2) == Some(&proof.payer.into_word())
    ).unwrap();
    let mut extra_credit = receipt.clone();
    extra_credit.logs.push(incoming.clone());
    assert!(fill(proof, &extra_credit, router).is_none());

    // Сверка решения 413 оставляет добавочный остаток другой сделке.
    let peer = req.qty;
    let shares = crate::core::credit_rest::share_out(qty + peer,
        &[(qty, Decimal::ZERO), (peer, Decimal::ZERO)], 18);
    assert_eq!(shares, [qty, peer]);
}

/// Подготавливает тот же SpotFill, который получает ядро после канонической сверки.
pub(super) async fn through_core(proof: &Proof, r: &send::receipt::Receipt, router: Address, log: &serde_json::Value) {
    use crate::core::types::{SendMethod, TxOutcome};
    use crate::venues::bsc_dex::exec::chain_tx;
    let raw = fill(proof, r, router).unwrap();
    let minimum = num::to_decimal(proof.prop.as_ref().unwrap().minimum, 6).unwrap();
    let req = TradeRequest {
        venue: VenueId("eth_dex".into()),
        asset: Asset { ticker:"USDT".into(), contract:Some(proof.output), chain:Some(ChainId("ethereum".into())) },
        deal_id: proof.leg.to_string(), side:Side::Buy, qty:minimum,
        worst_price:num::to_decimal(raw.0,6).unwrap() / minimum, route:None, exit_position:false, remeasure:None,
    };
    let (coins, gross) = recovery::amounts(proof, &req, (6,6), raw).unwrap();
    // Симуляция не даёт сетевого nonce или уплаченной цены газа. Эти поля модели нужны лишь
    // для воспроизведения ветки DEX (chain.is_some()); для них не заявляется рыночное измерение.
    let tracked = send::track::Tracked {outcome:TxOutcome::Included,receipt:Some(r.clone()),cancel:None,unresolved:None};
    let tx = chain_tx::chain_tx(log["transactionHash"].as_str().unwrap().parse().unwrap(), 0,
        vec![proof.builder.clone()], SendMethod::Bundle, &tracked, None);
    let spot = recovery::spot(proof, req, tx, coins, gross, Decimal::ZERO).unwrap();
    crate::core::prop_slot_filled_qty_tests::exit(spot).await;
}
