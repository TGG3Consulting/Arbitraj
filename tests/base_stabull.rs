//! Канонический реестр и деньги кошелька Stabull, включая архивные границы точного входа и выхода.
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
fn capture(raw: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(raw)).unwrap()
}
fn delta(logs: &Value, token: &Value, payer: &str) -> (u128, u128) {
    let (mut paid, mut received) = (0, 0);
    for log in logs.as_array().unwrap() {
        if log["address"] != *token
            || log["topics"][0]
                != "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
        {
            continue;
        }
        let units =
            u128::from_str_radix(log["data"].as_str().unwrap().trim_start_matches("0x"), 16)
                .unwrap();
        if log["topics"][1].as_str().unwrap().ends_with(payer) {
            paid += units;
        }
        if log["topics"][2].as_str().unwrap().ends_with(payer) {
            received += units;
        }
    }
    (paid, received)
}
#[test]
fn successful_payments_preserve_wallet_limits_in_each_form() {
    let dir = capture(include_bytes!(
        "data/first_run_dex_2026-11-26/stabull_base_directory.json.gz"
    ));
    let pools = dir["pools"].as_array().unwrap();
    assert_eq!(pools.len(), 12);
    let cfg: Value = serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let profile = &cfg
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "stabull" && p["chain_id"] == 8453)
        .unwrap()["settings"];
    assert_eq!(profile["dex.kind"], "stabull");
    assert!(profile.get("dex.router").is_none());
    assert_eq!(
        profile["dex.factory_runtime_hash"],
        dir["factory"]["runtime"]
    );
    let mut counts = BTreeMap::new();
    let mut markets = BTreeSet::new();
    let mut differing = BTreeMap::new();
    for bytes in [
        include_bytes!("data/first_run_dex_2026-11-26/stabull_base_native_wallets.json.gz")
            .as_slice(),
        include_bytes!("data/first_run_dex_2026-11-26/stabull_base_protected_wallets.json.gz")
            .as_slice(),
    ] {
        let a = capture(bytes);
        assert_eq!(a["selector"], dir["anchor"]);
        if !a["module"].is_null() {
            assert_eq!(
                a["module"]["runtime"],
                profile["dex.module_runtime_hashes"][0]
            );
        }
        let payer = a["payer"].as_str().unwrap().trim_start_matches("0x");
        let rows = a["records"].as_array().unwrap();
        assert_eq!(rows.len(), pools.len());
        for (row, pool) in rows.iter().zip(pools) {
            assert_eq!(row["pool"], pool["registered"]);
            for case in row["cases"].as_array().unwrap() {
                for key in ["buy", "sell", "buy_target", "sell_target"] {
                    let v = &case[key];
                    if v["stage"] != "paid" {
                        continue;
                    }
                    let mode = case["mode"].as_str().unwrap().to_owned();
                    *counts.entry(mode.clone()).or_insert(0) += 1;
                    markets.insert(row["pool"].as_str().unwrap().to_owned());
                    let (spent, refund) = delta(&v["logs"], &v["input"], payer);
                    let (fee, credit) = delta(&v["logs"], &v["output"], payer);
                    let paid = spent - refund;
                    let net = credit - fee;
                    assert_eq!(paid.to_string(), v["paid"].as_str().unwrap());
                    assert_eq!(net.to_string(), v["net"].as_str().unwrap());
                    let want = v["exact_output"] == true;
                    let specified = v["specified"].as_str().unwrap().parse::<u128>().unwrap();
                    if want {
                        assert!(net >= specified);
                    } else {
                        assert_eq!(paid, specified);
                    }
                    if (if want { paid } else { net }).to_string() != v["quoted"].as_str().unwrap()
                    {
                        *differing.entry(mode).or_insert(0) += 1;
                    }
                    assert_eq!(v["min_or_cap_error"]["code"], 3);
                    assert_eq!(v["deadline_error"]["code"], 3);
                }
            }
        }
    }
    assert_eq!(markets.len(), 6);
    assert_eq!(
        counts,
        BTreeMap::from([
            ("module".into(), 40),
            ("pool".into(), 40),
            ("router".into(), 20),
            ("v5".into(), 20)
        ])
    );
    assert!(differing.get("module").copied().unwrap_or(0) > 0);
}
