//! Проверяет отдельно обычные токены venture, реальные отправки и предыдущий PoolId после миграции.
use alloy_primitives::U256;
use serde_json::Value;
use std::collections::BTreeSet;

fn archive(bytes: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap()
}

fn balance(raw: &Value) -> U256 {
    assert_eq!(raw["status"], "0x1");
    U256::from_str_radix(
        raw["returnData"].as_str().unwrap().trim_start_matches("0x"),
        16,
    )
    .unwrap()
}

#[test]
fn hub_spot_vaults_and_wallets_match_physical_currencies() {
    let dir = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/umia_base_directory.json.gz"
    ));
    let history = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/umia_base_history.json.gz"
    ));
    let pay = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/umia_base_wallets.json.gz"
    ));
    assert_eq!(dir["selector"], history["selector"]);
    assert_eq!(dir["selector"], pay["selector"]);
    assert_eq!(dir["selector"]["requireCanonical"], true);
    let current = dir["rows"].as_array().unwrap();
    assert_eq!(current.len(), 7);
    assert_eq!(current.iter().filter(|p| p["id"].is_string()).count(), 4);
    let old = &history["retired"][0];
    let successor = current
        .iter()
        .find(|p| p["terms"]["migrator"].is_string())
        .unwrap();
    assert_ne!(old["id"], successor["id"]);
    assert_eq!(old["key"][0], successor["key"][0]);
    assert_eq!(old["key"][1], successor["key"][1]);
    assert_ne!(old["key"][2], successor["key"][2]);
    assert_ne!(old["liquidity"], "0");
    let mut accepted = BTreeSet::new();
    let mut empty = BTreeSet::new();
    for m in pay["markets"].as_array().unwrap() {
        let name = m["pool"].as_str().unwrap();
        let spot = current
            .iter()
            .chain(std::iter::once(old))
            .find(|r| r["id"] == name)
            .unwrap();
        assert_eq!(spot["key"], m["key"]);
        for case in m["cases"].as_array().unwrap() {
            if spot["liquidity"] == "0" {
                assert!(matches!(
                    case["buy"]["stage"].as_str(),
                    Some("quote" | "zero")
                ));
                assert!(case["sell"].is_null());
                empty.insert(name);
                continue;
            }
            for side in ["buy", "sell"] {
                let leg = &case[side];
                if leg["stage"] != "accepted" {
                    assert_eq!(spot["id"], old["id"]);
                    continue;
                }
                let results = leg["results"].as_array().unwrap();
                let n = results.len();
                assert_eq!(results[n - 5]["status"], "0x0");
                assert_eq!(results[n - 4]["status"], "0x0");
                assert_eq!(results[n - 3]["status"], "0x1");
                let paid = balance(&results[n - 8]) - balance(&results[n - 2]);
                let net = balance(&results[n - 1]) - balance(&results[n - 7]);
                assert_eq!(paid.to_string(), leg["input"]);
                assert_eq!(paid.to_string(), leg["paid"]);
                assert_eq!(net.to_string(), leg["net"]);
                assert_eq!(net.to_string(), leg["quoted"]);
                accepted.insert((
                    name,
                    case["mode"].as_str().unwrap(),
                    case["amount"].as_str().unwrap(),
                    side,
                ));
            }
        }
    }
    assert_eq!(empty.len(), 2);
    for spot in current
        .iter()
        .filter(|p| p["id"].is_string() && p["liquidity"] != "0")
    {
        assert_eq!(accepted.iter().filter(|p| p.0 == spot["id"]).count(), 16);
    }
}
