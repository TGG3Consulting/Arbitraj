//! Первичные квитанции Ethereum и настоящие независимые кадры двух строителей.
//! https://docs.titanbuilder.xyz/propamms · https://buildernet.org/docs/api .
use super::*;
use alloy_primitives::keccak256;
use std::collections::BTreeSet;

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/fermi_ethereum.json"
    ))
    .unwrap()
}
fn hex128(v: &Value) -> u128 {
    u128::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}
fn word(data: &str, index: usize) -> u128 {
    u128::from_str_radix(&data[2 + 64 * index..2 + 64 * (index + 1)], 16).unwrap()
}

#[test]
fn builders_drop_irrelevant_overrides_and_expire_at_target() {
    let f = fixture();
    for (name, builder) in [
        ("Titan", Builder::Titan),
        ("BuilderNet", Builder::BuilderNet),
    ] {
        let frame = &f["frames"][name];
        let diff = &frame["stateDiff"];
        let row = json!({"maker":"Fermi","quoteAddress":ROUTER,"blockNumber":frame["target"],
             "slot":frame["slot"],"stateOverride":{(format!("{ENGINE:#x}")):{"stateDiff":diff,"balance":"0x100","nonce":"0x7"},
             (format!("{ROUTER:#x}")):{"code":"0x01"}}});
        let result = match builder {
            Builder::Titan => json!({"venues":[row]}),
            Builder::BuilderNet => json!({(format!("{ROUTER:#x}")):row,"slot":frame["slot"],
                "blockNumber":format!("0x{:x}",frame["target"].as_u64().unwrap())}),
        };
        let parsed = Frame::read(builder, &result).unwrap();
        assert_eq!(
            parsed.overrides(),
            json!({(format!("{ENGINE:#x}")):{"stateDiff":diff}})
        );
        let t = parsed.timestamp;
        assert!(parsed.current(parsed.target - 1, t - 12, t - 1));
        assert!(!parsed.current(parsed.target - 2, t - 12, t - 1));
        assert!(!parsed.current(parsed.target - 1, t - 24, t - 13));
        assert!(!parsed.current(parsed.target - 1, t - 12, t));
    }
    assert_ne!(
        f["frames"]["Titan"]["stateDiff"],
        f["frames"]["BuilderNet"]["stateDiff"]
    );
}

#[test]
fn all_exact_swaps_match_wallet_and_minimum_and_require_the_target() {
    let f = fixture();
    for name in ["Titan", "BuilderNet"] {
        let sim = &f["simulations"][name];
        let target = f["frames"][name]["target"].as_u64().unwrap();
        assert_eq!(hex128(&sim["parent"]["number"]) + 1, u128::from(target));
        let mut sides = BTreeSet::new();
        for case in sim["cases"].as_array().unwrap() {
            let v = &case["results"];
            let amount = case["amount"].as_str().unwrap().parse::<u128>().unwrap();
            let min = case["minimum"].as_str().unwrap().parse::<u128>().unwrap();
            assert!(sides.insert((case["token_in"].as_str().unwrap(), amount)));
            for key in ["quote", "swap"] {
                assert_eq!(v[key]["status"], "0x1");
                let data = v[key]["returnData"].as_str().unwrap();
                assert_eq!((word(data, 0), word(data, 1)), (amount, min));
            }
            assert_eq!(
                hex128(&v["before_in"]["returnData"]) - hex128(&v["after_in"]["returnData"]),
                amount
            );
            assert_eq!(
                hex128(&v["after_out"]["returnData"]) - hex128(&v["before_out"]["returnData"]),
                min
            );
            assert_eq!(case["min_plus_one"]["status"], "0x0");
            for condition in ["without_update", "next_slot"] {
                assert_eq!(case["conditions"][condition]["status"], "0x0");
                assert_eq!(
                    case["conditions"][condition]["error"]["message"],
                    "execution reverted: SP"
                );
            }
            let calldata = case["swap_data"].as_str().unwrap();
            let hash =
                keccak256(b"fermiSwapWithAllowances(address,address,int256,uint256,address)")
                    .to_string();
            assert_eq!(&calldata[..10], &hash[..10]);
            let args = format!("0x{}", &calldata[10..]);
            assert_eq!((word(&args, 2), word(&args, 3)), (amount, min));
            let swap = v["swap"]["logs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|l| l["address"] == f["router"])
                .unwrap();
            let data = swap["data"].as_str().unwrap();
            assert_eq!((word(data, 0), word(data, 1)), (amount, min));
            for (i, token) in [(2, "token_in"), (3, "token_out")] {
                assert_eq!(
                    &swap["topics"][i].as_str().unwrap()[26..],
                    &case[token].as_str().unwrap()[2..]
                );
            }
        }
        assert_eq!(sides.len(), 4);
    }
}

#[test]
fn historical_receipts_prove_three_distinct_routers_and_vault_flows() {
    let f = fixture();
    let mut routers = BTreeSet::new();
    for x in f["historical"].as_array().unwrap() {
        let ev = &x["event"];
        assert_eq!(ev["blockHash"], x["block"]["hash"]);
        assert_eq!(ev["transactionHash"], x["transaction"]);
        assert_eq!(ev["address"], x["router"]);
        assert!(routers.insert(x["router"].as_str().unwrap()));
        if x["router"] == f["router"] {
            assert_eq!(x["runtime_keccak"], RUNTIME.to_string());
            assert_eq!(x["engine"], f["engine"]);
        }
        let data = ev["data"].as_str().unwrap();
        for (i, t) in x["transfers"].as_array().unwrap().iter().enumerate() {
            assert_eq!(word(data, i), hex128(&t["data"]));
            assert_eq!(t["transactionHash"], x["transaction"]);
            assert_eq!(
                &ev["topics"][i + 2].as_str().unwrap()[26..],
                &t["address"].as_str().unwrap()[2..]
            );
            assert_eq!(
                &t["topics"][if i == 0 { 2 } else { 1 }].as_str().unwrap()[26..],
                &x["trader_vault"].as_str().unwrap()[2..]
            );
        }
    }
    assert_eq!(routers.len(), 3);
    assert_eq!(
        f["window"]["event_counts"][f["router"].as_str().unwrap()],
        131
    );
}

#[test]
fn amounts_reject_inexact_units_and_signed_overflow() {
    assert_eq!(
        exact_amount(None, Some("1.000001"), 6),
        Some(U256::from(1000001))
    );
    assert_eq!(exact_amount(Some("1"), None, 18), Some(U256::from(1)));
    assert_eq!(exact_amount(None, Some("0.0000001"), 6), None);
    assert_eq!(exact_amount(Some("1"), Some("1"), 6), None);
    assert_eq!(exact_amount(Some(&U256::MAX.to_string()), None, 0), None);
}
