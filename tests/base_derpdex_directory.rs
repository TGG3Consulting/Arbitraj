//! Полный реестр фабрики DerpDEX Base на архивном блоке и фактическая глубина по направлению и размеру.
use std::collections::BTreeSet;

use alloy::sol_types::SolValue;
use alloy_primitives::{Address, B256, U256, keccak256};
use serde_json::Value;

fn directory() -> Value {
    serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/derpdex_directory.json"
    ))
    .unwrap()
}

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn deployment_directory_covers_factory_creations_and_all_configured_fee_tiers() {
    let dir = directory();
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let config = &profiles
        .iter()
        .find(|p| p["network"] == "base" && p["id"] == "derpdex-base")
        .unwrap()["settings"];
    let archive: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/derpdex_base.json"
    ))
    .unwrap();
    let init: B256 = archive["provenance"]["pool"]["actual"]["init_hash"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let factory = addr(&dir["factory"]);
    assert_eq!(addr(&config["dex.factory"]), factory);
    let tiers: BTreeSet<_> = config["dex.v3_fee_tiers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        tiers,
        dir["tiers"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect()
    );
    let mut deployed = BTreeSet::new();
    let mut events = BTreeSet::new();
    for row in dir["pools"].as_array().unwrap() {
        let fee = row["fee"].as_u64().unwrap();
        assert_eq!(dir["tiers"][fee.to_string()], row["spacing"]);
        let a = addr(&row["tokens"][0]);
        let b = addr(&row["tokens"][1]);
        assert!(a < b);
        let salt = keccak256((a, b, U256::from(fee)).abi_encode());
        assert_eq!(factory.create2(salt, init), addr(&row["pool"]));
        assert!(deployed.insert(addr(&row["pool"])));
        assert!(events.insert((row["tx"].as_str().unwrap(), row["log"].as_u64().unwrap())));
    }
    assert_eq!(deployed.len(), 13);
    assert_eq!(dir["nonce"].as_u64().unwrap(), 1 + deployed.len() as u64);
    assert_eq!(
        dir["storage_slots"].as_u64().unwrap(),
        (2 * deployed.len() + 1 + tiers.len()) as u64
    );
    let indexed: BTreeSet<_> = dir["indexed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| addr(&p["pool"]))
        .collect();
    assert_eq!(indexed.len(), 6);
    assert!(indexed.is_subset(&deployed));
}
