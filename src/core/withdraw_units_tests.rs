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
