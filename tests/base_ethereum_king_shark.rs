use alloy_primitives::{Address, B256};
use arb::venues::bsc_dex::pools::verify::v2_create2;
use flate2::read::GzDecoder;
use serde_json::Value;
use std::{fs::File, path::Path};
#[path = "common/first_run_profiles.rs"]
mod first_run_profiles;

fn archive(file: &str) -> Value {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/first_run_dex_2026-11-26");
    serde_json::from_reader(GzDecoder::new(File::open(root.join(file)).unwrap())).unwrap()
}
fn address(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn king_is_the_union_of_three_existing_sectorone_registries_on_each_chain() {
    let sources: Vec<Value> = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/king_sharkswap_source_bindings.json"
    ))
    .unwrap();
    for (network, chain, expected) in [("base", 8453, [0, 211, 43]), ("ethereum", 1, [0, 8, 11])] {
        let bound = sources
            .iter()
            .find(|r| r["network"] == network && r["id"] == "7304")
            .unwrap();
        let scopes = bound["scopes"].as_array().unwrap();
        assert_eq!(scopes.len(), 3);
        for (scope, count) in scopes.iter().zip(expected) {
            let profile =
                first_run_profiles::named(chain, scope["profile"].as_str().unwrap()).unwrap();
            assert_eq!(scope["factory"], profile["settings"]["dex.factory"]);
            let proof = archive(scope["evidence"].as_str().unwrap());
            if scope["generation"] == "V2" {
                assert_eq!(proof["original_count"], 0);
                assert_eq!(proof["factory"], scope["factory"]);
                assert_eq!(scope["paid_legs"], 0);
            } else {
                assert_eq!(proof["directory"]["factory"], scope["factory"]);
                assert_eq!(
                    proof["directory"]["markets"].as_array().unwrap().len(),
                    count
                );
                let paid = proof["settlement"]["cases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|c| [&c["buy"], &c["sell"]])
                    .filter(|v| {
                        v["net"]
                            .as_str()
                            .and_then(|s| s.parse::<u128>().ok())
                            .is_some_and(|n| n > 0)
                    })
                    .count();
                assert!(paid > 0);
                assert_eq!(scope["paid_legs"], paid);
            }
            assert_eq!(scope["registered"], count);
        }
    }
}

#[test]
fn sharkswap_router_and_v5_pay_identical_wallet_amounts_and_preserve_failed_limits() {
    let d = archive("sharkswap_base_wallets.json.gz");
    let profile = first_run_profiles::named(8453, "sharkswap-base").unwrap();
    let s = &profile["settings"];
    assert_eq!(s["dex.kind"], "v2_fork_dual_create2");
    assert_eq!(s["dex.factory"], d["roles"]["factory"]["address"]);
    assert_eq!(s["dex.router"], d["roles"]["router"]["address"]);
    assert_eq!(
        s["dex.router_runtime_hash"],
        d["roles"]["router"]["runtime"]
    );
    assert_eq!(s["dex.pool_code_hashes"][0], d["init"]);
    let init: B256 = d["init"].as_str().unwrap().parse().unwrap();
    let pools = d["markets"].as_array().unwrap();
    assert_eq!(pools.len(), 52);
    for m in pools {
        assert_eq!(
            v2_create2(
                address(&s["dex.factory"]),
                address(&m["tokens"][0]),
                address(&m["tokens"][1]),
                init
            ),
            address(&m["pool"])
        );
        assert_eq!(m["runtime"], d["roles"]["pair"]["runtime"]);
    }
    let mut paid = 0;
    for m in d["examples"].as_array().unwrap() {
        let cases = m["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 4);
        for a in cases.iter().filter(|v| v["mode"] == "v5") {
            let b = cases
                .iter()
                .find(|c| c["mode"] == "router" && c["size"] == a["size"])
                .unwrap();
            for leg in ["buy", "sell"] {
                for row in [a, b] {
                    assert_eq!(row[leg]["exact"]["status"], "0x1");
                    for bad in ["above", "expired"] {
                        assert_eq!(row[leg][bad]["status"], "0x0");
                        assert_eq!(row[leg][bad]["spent"], "0");
                        assert_eq!(row[leg][bad]["received"], "0");
                    }
                    paid += 1;
                }
                assert_eq!(a[leg]["exact"]["spent"], b[leg]["exact"]["spent"]);
                assert_eq!(a[leg]["exact"]["received"], b[leg]["exact"]["received"]);
            }
        }
    }
    assert_eq!(paid, 16);
}
