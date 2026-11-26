use alloy_primitives::{Address, B256, U256, keccak256};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::{fs::File, path::Path};

fn load(id: &str) -> Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "tests/data/first_run_dex_2026-11-26/base_algebra_{id}.json.gz"
    ));
    serde_json::from_reader(GzDecoder::new(File::open(p).unwrap())).unwrap()
}
fn number(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn word(results: &Value, key: &str) -> U256 {
    let data = results[key]["returnData"]
        .as_str()
        .unwrap()
        .strip_prefix("0x")
        .unwrap();
    U256::from_str_radix(data, 16).unwrap()
}
#[test]
fn base_plugins_create_the_configured_pool_and_match_real_wallet_deltas() {
    let cfg: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    for id in ["henjin", "kim-v4-base", "synthswap-v3", "treble-v4"] {
        let data = load(id);
        if id == "henjin" || id == "kim-v4-base" {
            let f = &cfg
                .iter()
                .find(|x| x["id"] == id && x["chain_id"] == 8453)
                .unwrap()["settings"];
            let roles = &data["roles"];
            assert_eq!(f["dex.kind"], "algebra_integral_base_plugin_v1");
            assert_eq!(f["dex.factory"], roles["factory"]["address"]);
            assert_eq!(f["dex.factory_runtime_hash"], roles["factory"]["keccak"]);
            assert_eq!(f["dex.plugin_factory"], roles["plugin_factory"]["address"]);
            assert_eq!(
                f["dex.plugin_runtime_hashes"][0],
                roles["plugin_factory"]["keccak"]
            );
            let init = f["dex.pool_code_hashes"][0]
                .as_str()
                .unwrap()
                .parse::<B256>()
                .unwrap();
            let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
            let mut salt = [0; 64];
            salt[12..32].copy_from_slice(addr(&data["tokens"][0]).as_slice());
            salt[44..].copy_from_slice(addr(&data["tokens"][1]).as_slice());
            assert_eq!(
                addr(&roles["deployer"]["address"]).create2(keccak256(salt), init),
                addr(&roles["pool"]["address"])
            );
            assert_eq!(data["global_state"][3], 193);
            assert_eq!(data["global_state"][5], true);
            assert_eq!(
                &data["reads"]["pluginByPool(address)"].as_str().unwrap()[26..],
                &roles["plugin"]["address"].as_str().unwrap()[2..]
            );
            assert_eq!(
                U256::from_str_radix(&data["reads"]["incentive()"].as_str().unwrap()[2..], 16)
                    .unwrap(),
                U256::ZERO
            );
        }
        let trials = data["cases"].as_array().unwrap();
        assert_eq!(trials.len(), 2);
        for leg in trials {
            let r = &leg["results"];
            let input = number(&leg["amount"]);
            let bought = number(&leg["bought"]);
            let sold = number(&leg["sold"]);
            assert_eq!(word(r, "before_in") - word(r, "buy_in"), input);
            assert_eq!(word(r, "buy_out") - word(r, "before_out"), bought);
            assert_eq!(word(r, "buy_out") - word(r, "sell_in"), bought);
            assert_eq!(word(r, "sell_out") - word(r, "buy_in"), sold);
            assert_eq!(word(r, "buy"), bought);
            assert_eq!(word(r, "sell"), sold);
            assert_eq!(word(r, "left_target"), U256::ZERO);
            assert_eq!(word(r, "left_source"), U256::ZERO);
            assert_eq!(r["above_buy"]["status"], "0x0");
            assert_eq!(r["above_sell"]["status"], "0x0");
            if id == "henjin" || id == "kim-v4-base" {
                let encoded = r["after_global"]["returnData"].as_str().unwrap();
                let actual = U256::from_str_radix(&encoded[130..194], 16).unwrap();
                assert_eq!(
                    actual,
                    U256::from_str_radix(
                        &data["reads"]["getCurrentFee()"].as_str().unwrap()[2..],
                        16
                    )
                    .unwrap()
                );
            }
        }
    }
}
