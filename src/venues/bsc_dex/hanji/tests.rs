//! Сверки на реальном объёме cbBTC/USDC из архива Hanji.
use super::{quantity::limit_price, routing::eligible_baseline};
use crate::core::types::{
    Asset, ChainId, Cost, CostKind, DexRoute, Quote, RouteMode, Side, VenueId,
};
use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::Value;
#[test]
fn archive_fill_requires_one_executor_event_and_each_actual_wallet_movement() {
    use super::{
        recovery::fill,
        wire::{Filled, Proof, executeExactCall},
    };
    use alloy::{
        rpc::types::Log,
        sol_types::{SolCall, SolEvent},
    };
    use alloy_primitives::{B256, Bytes};
    let archive: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/hanji_executor_receipts.json"
    ))
    .unwrap();
    assert_eq!(
        archive["runtime_keccak"],
        include_str!("runtime.txt").trim()
    );
    let cases = archive["evidence"].as_array().unwrap();
    assert_eq!(cases.len(), 2);
    for (i, row) in cases.iter().enumerate() {
        let p: Proof = serde_json::from_value(row["order"].clone()).unwrap();
        let logs: Vec<Log> = serde_json::from_value(row["logs"].clone()).unwrap();
        let value: Bytes = serde_json::from_value(row["return_data"].clone()).unwrap();
        let actual = executeExactCall::abi_decode_returns(&value).unwrap();
        assert_eq!(fill(&p, &logs), Some((actual.spent, actual.received)));
        if i == 0 {
            assert!(p.budget > actual.spent);
        }
        let mut wrong = p.clone();
        wrong.leg = B256::ZERO;
        assert!(fill(&wrong, &logs).is_none());
        wrong = p.clone();
        wrong.payer = p.executor;
        assert!(fill(&wrong, &logs).is_none());
        let event = logs
            .iter()
            .find(|l| l.topics().first() == Some(&Filled::SIGNATURE_HASH))
            .unwrap();
        assert!(fill(&p, std::slice::from_ref(event)).is_none());
        let mut doubled = logs.clone();
        doubled.push(event.clone());
        assert!(fill(&p, &doubled).is_none());
        let remaining = logs
            .iter()
            .filter(|l| l.address() != p.output)
            .cloned()
            .collect::<Vec<_>>();
        assert!(fill(&p, &remaining).is_none());
    }
}
#[test]
fn mixed_baseline_with_an_extra_approval_cannot_hide_funded_hanji() {
    let archive: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/hanji_base.json"
    ))
    .unwrap();
    let market = &archive["markets"][0];
    let token = &market["registry"]["baseToken"];
    let fill = &market["fills"][0];
    let qty = Decimal::new(fill["received_x"].as_i64().unwrap(), 8);
    let spent = Decimal::new(fill["debited_y"].as_i64().unwrap(), 6);
    let mut q = Quote {
        venue: VenueId("base_dex".into()),
        asset: Asset {
            ticker: token["symbol"].as_str().unwrap().into(),
            contract: Some(token["contractAddress"].as_str().unwrap().parse().unwrap()),
            chain: Some(ChainId("base".into())),
        },
        side: Side::Buy,
        qty,
        fillable_qty: qty,
        avg_price: spent / qty,
        costs: vec![],
        at: chrono::Utc::now(),
        signal_route: None,
        pick: None,
        route: Some(DexRoute {
            mode: RouteMode::Direct,
            legs: vec![],
            block: archive["block"].as_u64().unwrap(),
            gas_units: 0,
            executable: true,
            exec_note: None,
        }),
    };
    assert!(eligible_baseline(&q, qty, true, true));
    q.costs
        .push(Cost::new(CostKind::ApproveGas, Decimal::ZERO, "ETH", false));
    assert!(!eligible_baseline(&q, qty, true, true));
    assert!(eligible_baseline(&q, qty, true, false));
    q.costs.clear();
    q.route = None;
    assert!(!eligible_baseline(&q, qty, true, true));
    let from_helper = market["asks"][0][0].as_u64().unwrap() as u128;
    let actual_limit = 60231; // CI 45876356432, contract_ask, тот же рынок и блок.
    assert!(from_helper < actual_limit);
    assert!(
        limit_price(from_helper, U256::from(1_000_000_000_000_000u64), true).unwrap()
            >= actual_limit
    );
    assert!(limit_price(from_helper, U256::ZERO, true).unwrap() < actual_limit);
}
