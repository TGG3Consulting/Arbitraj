//! Тип маршрута в старых и новых карточках проверяется на параметрах Taurus Ethereum и Pancake StableSwap BSC.
use super::*;
use alloy::primitives::B256;
use alloy::sol_types::{SolEvent, SolValue};
use crate::venues::bsc_dex::pools::llamma::ILlama;
use serde_json::Value;

fn fixture(bytes: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap()
}

#[test]
fn changing_the_note_preserves_settlement_and_legacy_recovery_modes() {
    use crate::core::types::{ChainId, DexRoute, Quote, VenueId};
    use crate::venues::bsc_dex::{check::simulate::SimLog, num};
    use crate::gateway::quote_stage::settlement_quote_supported;

    let taurus = fixture(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/taurus_ethereum_bound_v2.json.gz"
    ));
    let cash = "0xdac17f958d2ee523a2206206994597c13d831ec7";
    let paid = taurus["builders"][0]["rows"].as_array().unwrap().iter()
        .find(|r| r["output"] == cash && r["withFrame"]["status"] == "paid").unwrap();
    let log = &paid["logs"][0];
    // В этом тесте хеш архивной симуляции служит непрозрачным идентификатором билета.
    let handle = serde_json::from_value::<B256>(log["transactionHash"].clone()).unwrap();
    let mut route = DexRoute {
        mode: RouteMode::PropSettlementSlot(Some(handle)),
        legs: vec![],
        block: u64::from_str_radix(log["blockNumber"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap(),
        gas_units: paid["withFrame"]["gas"].as_u64().unwrap(),
        executable: true,
        exec_note: None,
    };
    let qty = num::to_decimal(U256::from(paid["withFrame"]["paid"].as_u64().unwrap()), 18).unwrap();
    let received = num::to_decimal(U256::from(paid["withFrame"]["net"].as_u64().unwrap()), 6).unwrap();
    let mut q = Quote {
        venue: VenueId("eth_dex".into()),
        asset: Asset {ticker: "WETH".into(), chain: Some(ChainId("ethereum".into())), contract: Some(serde_json::from_value(paid["input"].clone()).unwrap())},
        side: Side::Sell,
        qty, fillable_qty: qty, avg_price: received / qty,
        costs: vec![], route: None, signal_route: None, pick: None, at: chrono::Utc::now(),
    };
    let bsc: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/bsc_pancake_stable_receipt_2026-11-27.json"
    )).unwrap();
    let old_note = "PropAMM: собственный однократный слот денежного шага";
    for (wire, mode, firm, size, recover) in [
        (serde_json::json!({"PropSettlementSlot": handle}), RouteMode::PropSettlementSlot(Some(handle)), true, true, Some(handle)),
        (serde_json::json!({"PropSettlementSlot": null}), RouteMode::PropSettlementSlot(None), false, true, None),
        (serde_json::json!({"Slot": handle}), RouteMode::Slot(handle), false, false, Some(handle)),
        (serde_json::json!("SlotIndicative"), RouteMode::SlotIndicative, false, false, None),
        (serde_json::json!("Direct"), RouteMode::Direct, true, true, None),
    ] {
        if mode == RouteMode::Direct {
            assert_eq!(bsc["chainId"], 56);
            route.block = bsc["blockNumber"].as_u64().unwrap();
            q.venue = VenueId("bsc_dex".into());
            q.asset = Asset {
                ticker: "USDT".into(), chain: Some(ChainId("bsc".into())),
                contract: Some(serde_json::from_value(bsc["poolTokens"][0].clone()).unwrap()),
            };
            let logs: Vec<SimLog> = serde_json::from_value(bsc["logs"].clone()).unwrap();
            let pool = serde_json::from_value::<Address>(bsc["pool"].clone()).unwrap();
            let event = logs.iter().find(|log| log.address == pool
                && log.topics.first() == Some(&ILlama::TokenExchange::SIGNATURE_HASH)).unwrap();
            let (i, sold, j, received) = <(U256, U256, U256, U256)>::abi_decode(&event.data).unwrap();
            assert_eq!((i, j), (U256::ZERO, U256::ONE));
            q.qty = num::to_decimal(sold, 18).unwrap();
            q.fillable_qty = q.qty;
            q.avg_price = num::to_decimal(received, 18).unwrap() / q.qty;
            route.gas_units = 0; // В BSC-записи сохранены события пула, газ не измерялся.
        }
        for note in [None, Some(old_note), Some("Изменённое пояснение")] {
            let mut stored = serde_json::to_value(&route).unwrap();
            stored["mode"] = wire.clone();
            stored["exec_note"] = serde_json::json!(note);
            let r: DexRoute = serde_json::from_value(stored).unwrap();
            assert_eq!(r.mode, mode);
            assert_eq!(serde_json::to_value(r.mode).unwrap(), wire);
            assert_eq!(r.mode.slot_ticket(), recover);
            q.route = Some(r);
            assert_eq!(settlement_quote_supported(&q, true), firm);
            assert_eq!(settlement_quote_supported(&q, false), size);
        }
    }
}
