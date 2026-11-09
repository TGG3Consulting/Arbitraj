//! Файл: tests/core_bnb_gas_236.rs
//! Что делает: решение 236, дополнение п. 8 (техлид 09.11 по поручению Тиграна) — сделка с монетой BNB: газ обмена DEX
//! (TradeResult.fees — газ в BNB, chain есть) идёт в затраты (spent), а не уменьшает количество монет — как в плане
//! (costs.rs: CostKind::Gas, convert::in_coin) и по решению 62. Раньше газ шёл видом TradeFee: у сделки с BNB
//! количество уменьшалось на газ, а при продаже BNB на DEX уходило в −газ — газ пропадал из итога.
//! Газ — из НАСТОЯЩЕЙ квитанции BSC (tests/data/receipts_2026-10-25/responses.json, results[2].result[1] — tx 0x6fa1…,
//! своп в блоке 129702236; та же, что в tests/bsc_dex_swap_resolve.rs): gasUsed × effectiveGasPrice (Receipt::gas_bnb).
//! Ядро — настоящая SQLite во временной папке без площадок (моков нет); venue.quote_asset у bsc_dex и mexc — USDT.
//! Цена BNB у сделки с BNB — цена этой же сделки (convert.rs: затрата в монете сделки — по её цене), живого стакана не
//! нужно — пересчёт в USDT проверяется числом.
//! Проверяется: (1) покупка BNB на DEX — количество = исполнено, spent = газ × цена сделки; (2) продажа BNB на DEX —
//! количество 0 (не −газ), spent = газ × цена; итог сделки (выручка − вложено − затраты) содержит газ ровно раз;
//! (3) биржа (chain нет): комиссия в BNB у сделки с BNB по-прежнему уменьшает количество (комиссия биржи из монет).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{fills, deal, types}, arb::venues::bsc_dex::send::receipt.

mod usdt_return_common;

use std::str::FromStr;

use alloy_primitives::B256;
use arb::core::deal::Deal;
use arb::core::types::{
    Asset, ChainTx, Direction, Fee, SendMethod, Side, TradeRequest, TradeResult, TradeStatus,
    TxOutcome, VenueId,
};
use arb::settings::value::Scope;
use arb::venues::bsc_dex::send::receipt::Receipt;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use usdt_return_common::{dir, start_with};

const RECEIPTS: &str = "tests/data/receipts_2026-10-25/responses.json";

/// Число из строки (тестовые литералы).
fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Газ настоящей квитанции свопа tx 0x6fa1… (BNB). Паника — файла нет.
fn real_gas() -> Decimal {
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(RECEIPTS).unwrap()).unwrap();
    let r: Receipt =
        serde_json::from_value(rec["results"][2]["response"]["result"][1].clone()).unwrap();
    let g = r.gas_bnb().unwrap();
    assert!(!g.is_zero());
    g
}

/// Монета BNB.
fn bnb() -> Asset {
    Asset {
        ticker: "BNB".into(),
        bsc_contract: None,
    }
}

/// Сделка BNB: покупка на buy, продажа на mexc.
fn deal(id: &str, buy: &str) -> Deal {
    let dir = Direction {
        asset: bnb(),
        buy_on: VenueId(buy.into()),
        sell_on: VenueId("mexc".into()),
    };
    Deal::new(id.into(), dir, json!(null), Decimal::ONE)
}

/// Итог исполнения: площадка, сторона, исполнено, цена, комиссия в BNB, есть ли транзакция в сети (DEX).
fn result(venue: &str, side: Side, qty: &str, price: &str, fee: Decimal, dex: bool) -> TradeResult {
    TradeResult {
        status: TradeStatus::Filled,
        filled_qty: d(qty),
        avg_price: d(price),
        fees: Fee::single(fee, "BNB"),
        venue_ref: "0x6fa1".into(),
        chain: dex.then(|| ChainTx {
            hash: B256::repeat_byte(6),
            nonce: 1,
            channels: vec![],
            method: SendMethod::Single,
            outcome: TxOutcome::Included,
            block: Some(129702236),
            gas_used: None,
            gas_bnb: Some(fee),
            tip_bnb: None,
            cancel_hash: None,
            cancel_gas_bnb: None,
            approve_gas_bnb: None,
            tip_hash: None,
            tip_gas_bnb: None,
        }),
        finished_at: chrono::Utc::now(),
        executor: None,
        request: TradeRequest {
            venue: VenueId(venue.into()),
            asset: bnb(),
            side,
            qty: d(qty),
            worst_price: d(price),
            deal_id: "t".into(),
            route: None,
            exit_position: false,
            remeasure: None,
        },
    }
}

/// Ядро с котируемой монетой USDT у bsc_dex (mexc, bingx — из общего старта).
async fn core(tag: &str) -> arb::core::Core {
    let extra = vec![(
        "venue.quote_asset",
        Scope::Venue("bsc_dex".into()),
        json!("USDT"),
    )];
    start_with(&dir(tag), None, extra).await.0
}

#[tokio::test]
async fn dex_gas_of_bnb_deal_is_spent_not_coins() {
    let core = core("a236bnb").await;
    let gas = real_gas();
    // (1) Покупка 0.5 BNB на DEX по 600: монет ровно 0.5 (газ — из резерва BNB кошелька), затраты = газ × 600.
    let mut x = deal("b1BNB", "bsc_dex");
    let buy = result("bsc_dex", Side::Buy, "0.5", "600", gas, true);
    arb::core::fills::trade(&core, &mut x, &buy).await;
    assert_eq!(x.qty, d("0.5"), "газ не уменьшает количество");
    assert_eq!(x.invested, d("300"));
    assert_eq!(
        x.spent,
        gas * d("600"),
        "газ — в затратах, по цене BNB этой сделки"
    );

    // (2) Продажа 0.5 BNB на DEX по 610 (деньги пришли на кошелёк): количество 0, не −газ; газ — в затратах.
    let mut y = deal("b2BNB", "mexc");
    y.at = VenueId("bsc_dex".into());
    y.qty = d("0.5");
    y.invested = d("300");
    let sell = result("bsc_dex", Side::Sell, "0.5", "610", gas, true);
    arb::core::fills::trade(&core, &mut y, &sell).await;
    assert_eq!(y.qty, Decimal::ZERO, "раньше: −газ, газ пропадал из итога");
    assert_eq!(y.proceeds, d("305"));
    assert_eq!(y.spent, gas * d("610"));
    // Итог: выручка − вложено − затраты — газ в нём ровно один раз.
    assert_eq!(y.result_if(Decimal::ZERO), d("5") - gas * d("610"));
}

#[tokio::test]
async fn cex_fee_in_bnb_still_reduces_coins() {
    let core = core("a236bnbcex").await;
    // (3) Биржа: комиссия 0.0005 BNB с покупки BNB — из монет (chain нет), в затраты не идёт.
    let mut x = deal("b3BNB", "mexc");
    let buy = result("mexc", Side::Buy, "0.5", "600", d("0.0005"), false);
    arb::core::fills::trade(&core, &mut x, &buy).await;
    assert_eq!(x.qty, d("0.4995"));
    assert_eq!(x.spent, Decimal::ZERO);
}
