//! Archived settlement of the third-token fee from proceeds, using the same owner's input budget.
use alloy_primitives::U256;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn read(bytes: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap()
}
fn amount(value: &Value) -> U256 {
    value.as_str().unwrap().parse().unwrap()
}
#[test]
fn registered_markets_and_both_sources_of_working_capital_are_distinct() {
    let proof = read(include_bytes!(
        "data/first_run_dex_2026-11-26/antfarm_output_ethereum.json.gz"
    ));
    let registry = read(include_bytes!(
        "data/first_run_dex_2026-11-26/antfarm_ethereum_registry.json.gz"
    ));
    assert_eq!(proof["selector"], registry["selector"]);
    assert_eq!(proof["state_root"], registry["state_root"]);
    let rows = proof["coverage"].as_array().unwrap();
    let pools: BTreeSet<_> = rows.iter().map(|r| r["pair"].as_str().unwrap()).collect();
    assert_eq!(pools.len(), 39);
    assert!(
        registry["pairs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| pools.contains(p["pair"].as_str().unwrap()))
    );
    assert_eq!(
        rows.iter()
            .filter(|o| o["class_name"] == "separate fee")
            .count(),
        20
    );
    assert_eq!(amount(&proof["fee_percentage"]), U256::ZERO);
    let mut routes = BTreeMap::new();
    for case in proof["cases"].as_array().unwrap() {
        let modes = routes
            .entry((case["pair"].as_str().unwrap(), amount(&case["qty"])))
            .or_insert_with(BTreeMap::new);
        let loan = case["loan"].as_bool().unwrap();
        assert!(modes.insert(loan, case).is_none());
        let gross = amount(&case["gross"]);
        let cost = amount(&case["cost"]);
        let credit = amount(&case["credit"]);
        assert_eq!(credit + cost, gross);
        let calls = case["calls"].as_array().unwrap();
        let rs = case["results"].as_array().unwrap();
        assert_eq!(calls.len(), rs.len());
        let idx = calls
            .iter()
            .position(|v| v["to"] == proof["module"])
            .unwrap();
        let width = rs.len() - 1 - idx;
        let before = &rs[idx - width..idx];
        let after = &rs[idx + 1..];
        let val = |r: &Value| {
            assert_eq!(r["status"], "0x1");
            amount(&r["returnData"])
        };
        assert_eq!(val(&before[0]) - val(&after[0]), amount(&case["qty"]));
        assert_eq!(val(&after[1]) - val(&before[1]), credit);
        for (b, a) in before.iter().zip(after).skip(2) {
            assert_eq!(val(b), val(a));
        }
        assert_eq!(case["controls"].as_array().unwrap().len(), 5);
        assert!(
            case["controls"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["result"]["status"] == "0x0"
                    && c["result"]["logs"].as_array().unwrap().is_empty())
        );
        if loan {
            assert_eq!(val(&before[2]), U256::ZERO);
        } else {
            assert!(val(&before[2]) >= amount(&case["atf"]));
        }
    }
    assert_eq!(routes.len(), 4);
    for modes in routes.values() {
        let stock = modes[&false];
        let loan = modes[&true];
        assert_eq!(stock["credit"], loan["credit"]);
        assert_eq!(stock["cost"], loan["cost"]);
        assert_eq!(stock["atf"], loan["atf"]);
        assert!(stock["gas"].as_u64().unwrap() < loan["gas"].as_u64().unwrap());
    }
}
