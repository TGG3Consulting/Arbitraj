//! Архив Base запускает каждую версию хука с официальным роутером и проверяет
//! действительное зачисление, списание и оба отрицательных ограничения по тем же calldata.
use alloy_primitives::{B256, Bytes, U256, keccak256};
use serde_json::Value;
use std::collections::BTreeSet;

fn unpack(raw: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(raw)).unwrap()
}
fn number(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn returns(row: &Value) -> U256 {
    number(&row["returnData"])
}

#[test]
fn configured_keys_match_initialize_and_verified_role_code() {
    let data = unpack(include_bytes!(
        "data/first_run_dex_2026-11-26/base_launch_hooks.json.gz"
    ));
    let config: Value = serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    for (id, count) in [("bankr", 15), ("bankr-index-base", 9), ("o1-launchpad", 8)] {
        let cfg = &config
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id && r["chain_id"] == 8453)
            .unwrap()["settings"];
        let ids: BTreeSet<_> = cfg["dex.launch_pool_keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let raw: Bytes = r.as_str().unwrap().parse().unwrap();
                assert_eq!(raw.len(), 160);
                keccak256(raw)
            })
            .collect();
        let source: BTreeSet<_> = data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["group"] == id && r["key"].is_array())
            .map(|r| {
                assert_eq!(r["log"]["address"], data["manager"]);
                assert_eq!(r["log"]["topics"][1], r["id"]);
                assert_eq!(
                    data["contracts"][r["key"][4].as_str().unwrap()]["runtime"],
                    r["hook_hash"]
                );
                r["id"].as_str().unwrap().parse::<B256>().unwrap()
            })
            .collect();
        assert_eq!(ids, source);
        assert_eq!(ids.len(), count);
        for role in cfg["dex.launch_roles"].as_array().unwrap() {
            let fields: Vec<_> = role.as_str().unwrap().split('@').collect();
            assert_eq!(fields.len(), 3);
            assert_eq!(data["role_codes"][fields[1]]["hash"], fields[2]);
            assert!(data["role_codes"][fields[1]]["size"].as_u64().unwrap() > 0);
        }
    }
}

#[test]
fn positive_and_failed_orders_match_raw_wallet_deltas() {
    for (raw, count, expected) in [
        (
            include_bytes!("data/first_run_dex_2026-11-26/o1_wrapped_base_wallets.json.gz")
                .as_slice(),
            3,
            24,
        ),
        (
            include_bytes!("data/first_run_dex_2026-11-26/baseline_base_wallets.json.gz")
                .as_slice(),
            1,
            8,
        ),
        (
            include_bytes!("data/first_run_dex_2026-11-26/bankr_base_wallets.json.gz").as_slice(),
            15,
            120,
        ),
        (
            include_bytes!("data/first_run_dex_2026-11-26/bankr_index_base_base_wallets.json.gz")
                .as_slice(),
            9,
            56,
        ),
        (
            include_bytes!("data/first_run_dex_2026-11-26/o1_launchpad_base_wallets.json.gz")
                .as_slice(),
            8,
            52,
        ),
    ] {
        let data = unpack(raw);
        let mut accepted = 0;
        let mut markets = BTreeSet::new();
        for m in data["markets"].as_array().unwrap() {
            for c in m["cases"].as_array().unwrap() {
                for leg in ["buy", "sell"] {
                    let proof = &c[leg];
                    if proof["stage"] != "accepted" {
                        continue;
                    }
                    let r = proof["results"].as_array().unwrap();
                    let n = r.len();
                    assert_eq!(r[n - 5]["status"], "0x0");
                    assert_eq!(r[n - 4]["status"], "0x0");
                    assert_eq!(r[n - 3]["status"], "0x1");
                    assert_eq!(
                        returns(&r[n - 8]) - returns(&r[n - 2]),
                        number(&proof["input"])
                    );
                    assert_eq!(
                        returns(&r[n - 1]) - returns(&r[n - 7]),
                        number(&proof["quoted"])
                    );
                    assert_eq!(proof["paid"], proof["input"]);
                    assert_eq!(proof["net"], proof["quoted"]);
                    markets.insert(m["pool"].as_str().unwrap());
                    accepted += 1;
                }
            }
        }
        assert_eq!((markets.len(), accepted), (count, expected));
    }
}
