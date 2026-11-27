//! Пересчитывает списание и выплату с архивных балансов для каждого полного обмена Spectra.
use alloy_primitives::U256;
use serde_json::Value;
use std::collections::BTreeSet;

fn value(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn balance(result: &Value) -> U256 {
    assert_eq!(result["status"], "0x1");
    U256::from_str_radix(
        result["returnData"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
        16,
    )
    .unwrap()
}

#[test]
fn both_wallet_assets_and_strict_controls_match_in_live_and_matured_markets() {
    let bytes = include_bytes!("data/first_run_dex_2026-11-26/spectra_base_wallets.json.gz");
    let archive: Value =
        serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice())).unwrap();
    let mut v5 = 0;
    let mut direct = 0;
    let mut markets = BTreeSet::new();
    let mut matured = BTreeSet::new();
    for m in archive["markets"].as_array().unwrap() {
        for wallet in m["wallets"].as_array().unwrap() {
            let Some(legs) = wallet["legs"].as_array() else {
                continue;
            };
            for leg in legs.iter().filter(|l| l["stage"] == "accepted") {
                let mode = wallet["mode"].as_str().unwrap();
                let results = leg["results"].as_array().unwrap();
                let calls = leg["calls"].as_array().unwrap();
                let real = results.len() - 5;
                let controls = if mode == "pool" { 1 } else { 2 };
                let before = real - controls - 2;
                assert_eq!(results[real]["status"], "0x1");
                assert!(
                    results[before + 2..real]
                        .iter()
                        .all(|r| r["status"] == "0x0")
                );
                assert_eq!(calls[real]["from"], wallet["owner"]);
                assert_eq!(
                    balance(&results[before]) - balance(&results[real + 1]),
                    value(&leg["qty"])
                );
                assert_eq!(
                    balance(&results[real + 2]) - balance(&results[before + 1]),
                    value(&leg["gross"])
                );
                assert_eq!(leg["qty"], leg["paid"]);
                assert_eq!(leg["net"], leg["gross"]);
                if mode == "v5" {
                    assert_eq!(balance(&results[real + 3]), U256::ZERO);
                    assert_eq!(balance(&results[real + 4]), U256::ZERO);
                    v5 += 1;
                } else if mode == "pool" {
                    direct += 1;
                }
                markets.insert(m["pool"].as_str().unwrap());
                if m["maturity"].as_u64().unwrap() <= 1_795_642_926 {
                    matured.insert(m["pool"].as_str().unwrap());
                }
            }
        }
    }
    assert!(
        v5 >= 48
            && direct >= 48
            && markets.len() >= 12
            && matured.len() >= 10
            && markets.len() > matured.len()
    );
}
