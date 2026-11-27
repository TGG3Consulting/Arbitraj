//! Проверяет профили обнаружения пулов по доказанному хранилищу каждой физической фабрики.
use flate2::read::GzDecoder;
use serde_json::Value;
use std::{collections::BTreeSet, fs::File};

#[test]
fn each_positive_factory_fee_is_available_for_cold_pool_discovery() {
    let profiles: Vec<Value> =
        serde_json::from_reader(File::open("config/first_run_dex.json").unwrap()).unwrap();
    for (network, chain, expected_ids) in [
        ("base", 8453, ["iziswap-base", "kyberswap-elastic-base"]),
        ("ethereum", 1, ["bitswap", "kyberswap_elastic"]),
    ] {
        let path = format!("tests/data/first_run_dex_2026-11-26/fork_fee_registry_{network}.json.gz");
        let archive: Value =
            serde_json::from_reader(GzDecoder::new(File::open(path).unwrap())).unwrap();
        assert_eq!(archive["chain_id"], chain);
        assert_eq!(archive["selector"]["requireCanonical"], true);
        let rows = archive["factories"].as_array().unwrap();
        assert_eq!(rows.len(), expected_ids.len());
        for id in expected_ids {
            let row = rows.iter().find(|r| r["id"] == id).unwrap();
            let settings = &profiles.iter().find(|p| p["chain_id"] == chain && p["id"] == id).unwrap()["settings"];
            assert!(settings["dex.factory"].as_str().unwrap().eq_ignore_ascii_case(row["factory"].as_str().unwrap()));
            let tiers = row["tiers"].as_object().unwrap();
            let configured: BTreeSet<&str> = settings["dex.v3_fee_tiers"].as_array().unwrap().iter()
                .map(|v| v.as_str().unwrap()).collect();
            assert_eq!(configured, tiers.keys().map(String::as_str).collect());
            let markets = row["markets"].as_array().unwrap();
            let all: BTreeSet<_> = markets.iter().map(|m| m["pool"].as_str().unwrap()).collect();
            assert_eq!(all.len(), markets.len());
            let mut activated = BTreeSet::new();
            for at in row["activations"].as_array().unwrap() {
                let fee = at["fee"].as_u64().unwrap();
                assert!(activated.insert(fee));
                assert_eq!(at["before"], 0);
                assert_eq!(tiers[&fee.to_string()], at["spacing"]);
                assert!(at["block"].as_u64().unwrap() <= archive["block"].as_u64().unwrap());
                assert_ne!(at["hash"], at["parent"]);
                assert_eq!(at["tx"].is_string(), row["kind"] == "kyber");
            }
            assert_eq!(activated.len(), tiers.len());
            for m in markets {
                assert_eq!(tiers[&m["fee"].to_string()], m["spacing"]);
            }
            if id == "bitswap" {
                for fee in [100, 400, 2000] {
                    assert!(activated.contains(&fee));
                    assert!(markets.iter().all(|m| m["fee"] != fee));
                }
            }
        }
    }
}
