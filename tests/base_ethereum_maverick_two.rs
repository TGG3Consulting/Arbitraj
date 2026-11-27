//! Приёмка пресета Maverick V2 по канонической фабрике и измеренной выплате каждого заявленного входного актива.
use flate2::read::GzDecoder;
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::Path};

#[test]
fn direct_tokens_have_registration_matching_code_and_a_paid_wallet_sale() {
    let folder = Path::new("tests/data/first_run_dex_2026-11-26");
    let profiles: Vec<Value> =
        serde_json::from_slice(&fs::read("config/first_run_dex.json").unwrap()).unwrap();
    let tokens: Vec<Value> = serde_json::from_reader(
        fs::File::open(folder.join("maverick_v2_exact_tokens.json")).unwrap(),
    )
    .unwrap();
    for network in ["base", "ethereum"] {
        let compressed = |kind: &str| -> Value {
            serde_json::from_reader(GzDecoder::new(
                fs::File::open(folder.join(format!("maverick_v2_{network}_{kind}.json.gz")))
                    .unwrap(),
            ))
            .unwrap()
        };
        let snap = compressed("directory");
        let wallet = compressed("wallets");
        assert_eq!(wallet["selector"], snap["selector"]);
        let preset = &profiles
            .iter()
            .find(|p| p["network"] == network && p["settings"]["dex.kind"] == "maverick_v2_router")
            .unwrap()["settings"];
        let roles = &snap["roles"];
        assert_eq!(preset["dex.router"], roles["router"]["address"]);
        assert_eq!(
            preset["dex.factory_runtime_hash"],
            roles["factory"]["runtime"]
        );
        assert_eq!(
            preset["dex.router_runtime_hash"],
            roles["router"]["runtime"]
        );
        assert_eq!(preset["dex.tick_radius"], 32);
        let wrapped = roles["router"]["wrapped"].as_str().unwrap();
        let mut sold = BTreeSet::new();
        for m in wallet["independent"]["markets"].as_array().unwrap() {
            assert_eq!(
                m["tokens"],
                snap["markets"][m["pool"].as_str().unwrap()]["tokens"]
            );
            for a in m["attempts"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|a| a["mode"] == "router")
            {
                let leg = &a["sell"];
                if leg["exact"]["status"] == "0x1" {
                    assert_eq!(leg["above"]["status"], "0x0");
                    assert_eq!(leg["expired"]["status"], "0x0");
                    assert_eq!(leg["matches_quote"], true);
                    sold.insert(leg["input"].as_str().unwrap());
                }
            }
        }
        for entry in preset["dex.router_exact_tokens"].as_array().unwrap() {
            let (address, code) = entry.as_str().unwrap().split_once(' ').unwrap();
            if address == wrapped {
                continue;
            }
            assert!(sold.contains(address), "{network} {address}");
            let proof = tokens
                .iter()
                .find(|t| t["network"] == network && t["address"] == address)
                .unwrap();
            assert_eq!(proof["selector"], snap["selector"]);
            assert_eq!(proof["keccak256"], code);
        }
    }
}
