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
                assert_eq!(leg["stage"], "accepted");
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
    assert_eq!(accepted.len(), 48);
    assert_eq!(accepted.iter().filter(|p| p.0 == old["id"]).count(), 16);
    for spot in current
        .iter()
        .filter(|p| p["id"].is_string() && p["liquidity"] != "0")
    {
        assert_eq!(accepted.iter().filter(|p| p.0 == spot["id"]).count(), 16);
    }
}

#[test]
fn first_run_routes_match_canonical_spot_pools_and_payments() {
    let profiles: Value = serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let profile = profiles.as_array().unwrap().iter()
        .find(|p| p["id"] == "umia-base").unwrap();
    let cfg = &profile["settings"];
    let directory = archive(include_bytes!("data/first_run_dex_2026-11-26/umia_base_directory.json.gz"));
    let history = archive(include_bytes!("data/first_run_dex_2026-11-26/umia_base_history.json.gz"));
    let payments = archive(include_bytes!("data/first_run_dex_2026-11-26/umia_base_wallets.json.gz"));
    assert_eq!(profile["chain_id"], directory["chain_id"]);
    assert_eq!(cfg["dex.factory"], directory["hub"]);
    for role in ["dex.pool_manager", "dex.state_view", "dex.universal_router", "dex.permit2", "dex.quoter"] {
        assert_eq!(cfg[role], payments["roles"][role], "{role}");
    }
    let registered: BTreeSet<_> = directory["rows"].as_array().unwrap().iter()
        .chain(history["retired"].as_array().unwrap())
        .filter_map(|r| r["id"].as_str()).collect();
    let keys = cfg["dex.launch_pool_keys"].as_array().unwrap();
    let configured: BTreeSet<String> = keys.iter().map(|k| {
        let key: alloy_primitives::Bytes = k.as_str().unwrap().parse().unwrap();
        assert_eq!(key.len(), 5 * 32);
        alloy_primitives::keccak256(key).to_string()
    }).collect();
    assert_eq!(keys.len(), registered.len());
    assert_eq!(registered, configured.iter().map(String::as_str).collect());
    for role in cfg["dex.launch_roles"].as_array().unwrap() {
        let parts: Vec<_> = role.as_str().unwrap().split('@').collect();
        let [_, address, hash] = parts.as_slice() else { panic!("invalid role {role}") };
        let deployed = if *address == history["retired"][0]["vault"].as_str().unwrap() {
            &history["retired"][0]["code"]
        } else {
            &directory["contracts"][*address]
        };
        assert!(deployed["bytes"].as_u64().unwrap() > 0);
        assert_eq!(deployed["keccak256"].as_str().unwrap(), *hash);
    }
}
