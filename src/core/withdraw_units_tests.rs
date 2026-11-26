use super::*;
use crate::core::types::Transfer;
use serde_json::Value;

#[test]
fn only_the_proven_legacy_amounts_are_excluded_after_reload() {
    let rows: Vec<Value> = serde_json::from_str(include_str!(
        "../../tests/data/poloniex_withdraw_alias_2026-11-23/sndk_2026-11-24.json"
    ))
    .unwrap();
    for row in rows {
        let sent: Transfer = serde_json::from_value(row["sent"].clone()).unwrap();
        let recv: Transfer = serde_json::from_value(row["credited"].clone()).unwrap();
        let requested = row["withdraw_requested"].as_str().unwrap().parse().unwrap();
        let fee = sent.fee.unwrap().amount;
        let legacy = Seen {
            asset: recv.asset,
            from: sent.venue,
            sent: requested,
            credited: recv.amount,
            fee: Some(fee),
        };
        let converted = Seen {
            sent: sent.amount,
            credited: sent.amount - fee,
            ..legacy.clone()
        };
        let tx = recv.tx_hash.unwrap();
        let receipt = Receipt {
            venue_units: None,
            tx: tx.clone(),
            requested,
            raw: recv.amount,
        };
        assert!(receipt.matches(&legacy, &converted));
        let key = (legacy.asset.clone(), legacy.from.clone());
        let mut state = State::default();
        state.legacy.insert(key.clone(), legacy.clone());
        let stored = Row {
            to: recv.venue,
            fact: converted.clone(),
            receipt: Some(receipt),
            replaced: Some((legacy.clone(), tx)),
        };
        let restored = serde_json::from_value(serde_json::to_value(&stored).unwrap()).unwrap();
        state.put(&restored);
        assert_eq!(
            state.lookup(&key.0, &key.1, Some(&stored.to)),
            Some(converted)
        );
        assert_eq!(state.lookup(&key.0, &key.1, None), None);
        assert!(state.legacy.contains_key(&key));
        let unrelated = Seen {
            fee: None,
            ..legacy
        };
        state.legacy.insert(key.clone(), unrelated.clone());
        assert_eq!(state.lookup(&key.0, &key.1, None), Some(unrelated));
    }
}

#[test]
fn historic_tstbsc_costs_use_the_current_tst_unit_only_for_the_same_chain_contract() {
    use crate::core::types::{ChainId, Cost, CostKind};
    let addr = "0x86bb94ddd16efc8bc58e6b056e8df71d9e666429"
        .parse()
        .unwrap();
    let historical = Asset {
        ticker: "TSTBSC".into(),
        contract: Some(addr),
        chain: None,
    };
    let current = Asset {
        ticker: "TST".into(),
        ..historical.clone()
    };
    let seen = Seen {
        asset: historical,
        from: VenueId("mexc".into()),
        sent: "12551.25".parse().unwrap(),
        credited: "12541.25".parse().unwrap(),
        fee: None,
    };
    let old_fee = Decimal::ZERO;
    let external = Cost::new(CostKind::WithdrawFee, Decimal::ONE, "MX", false);
    let principal = Cost::new(CostKind::WithdrawFee, Decimal::ZERO, "TST", false);
    let old = super::super::model(
        &seen,
        Some(old_fee),
        Decimal::ZERO,
        seen.sent,
        vec![principal.clone()],
    );
    assert!(old.iter().all(|c| c.asset == "TSTBSC"));
    let model = |target: &Asset, dir: Vec<Cost>| {
        super::super::route_model(
            seen.clone(),
            target,
            Some(old_fee),
            Decimal::ZERO,
            seen.sent,
            dir,
        )
    };
    for _ in 0..2 {
        let costs = model(&current, vec![principal.clone()]);
        assert!(costs.iter().all(|c| c.asset == "TST"));
        assert_eq!(
            costs
                .iter()
                .find(|c| c.kind == CostKind::TransferShortfall)
                .unwrap()
                .amount
                .round_dp(8),
            Decimal::TEN
        );
    }
    for independent in [external, principal.clone().paid_with("MX")] {
        assert_eq!(
            model(&current, vec![independent.clone()]),
            vec![independent]
        );
    }
    for other in [
        Asset {
            chain: Some(ChainId("arbitrum".into())),
            ..current.clone()
        },
        Asset {
            contract: Some(alloy_primitives::Address::ZERO),
            ..current.clone()
        },
        Asset {
            contract: None,
            ..current.clone()
        },
    ] {
        assert_eq!(
            model(&other, vec![principal.clone()]),
            vec![principal.clone()]
        );
    }
}
