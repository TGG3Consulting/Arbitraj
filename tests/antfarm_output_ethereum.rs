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

#[test]
fn reverse_sale_uses_both_post_purchase_reserves_and_the_owners_remaining_tokens() {
    let proof = read(include_bytes!(
        "data/first_run_dex_2026-11-26/antfarm_output_ethereum.json.gz"
    ));
    let cases = proof["roundtrips"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    let mut samples = BTreeSet::new();
    for row in cases {
        let n = amount(&row["bought"]);
        let rest = n - n / U256::from(2) - n / U256::from(4);
        assert_eq!(rest, amount(&row["sold"]));
        assert!(samples.insert((row["pair"].as_str().unwrap(), n)));
        let reserves = row["post_reserves"].as_array().unwrap();
        assert_eq!(reserves.len(), 2);
        let rs = row["results"].as_array().unwrap();
        let calls = row["calls"].as_array().unwrap();
        assert_eq!(calls.len(), rs.len());
        assert_eq!(calls[5]["to"], row["input_module"]);
        assert_eq!(rs[5]["status"], "0x1");
        let val = |r: &Value| amount(&r["returnData"]);
        assert_eq!(val(&rs[2]) - val(&rs[6]), amount(&row["cash_budget"]));
        assert_eq!(val(&rs[7]) - val(&rs[3]), n);
        assert_eq!(val(&rs[4]), val(&rs[8]));
        for (words, value) in rs[9..11].iter().zip(reserves) {
            let hex = words["returnData"]
                .as_str()
                .unwrap()
                .strip_prefix("0x")
                .unwrap();
            for (i, reserve) in value.as_array().unwrap().iter().enumerate() {
                assert_eq!(
                    U256::from_str_radix(&hex[64 * i..64 * (i + 1)], 16).unwrap(),
                    amount(reserve)
                );
            }
        }
        assert_eq!(calls[rs.len() - 4]["to"], proof["module"]);
        let tail = &rs[rs.len() - 8..];
        assert_eq!(tail[3]["status"], "0x0");
        assert!(tail[3]["logs"].as_array().unwrap().is_empty());
        assert_eq!(tail[4]["status"], "0x1");
        assert_eq!(val(&tail[0]) - val(&tail[5]), rest);
        assert_eq!(val(&tail[6]) - val(&tail[1]), amount(&row["net"]));
        assert_eq!(val(&tail[2]), val(&tail[7]));
    }
    assert!(cases.iter().any(|r| r["earlier_cost"] != r["after_cost"]));
}
