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
fn limit_market_alias_reconciles_two_cash_sizes_on_each_underlying_pool() {
    let read = |raw: &[u8]| -> Value { serde_json::from_reader(GzDecoder::new(raw)).unwrap() };
    let replay = read(include_bytes!(
        "data/first_run_dex_2026-11-26/treble_limit_wallets.json.gz"
    ));
    let registry = read(include_bytes!(
        "data/first_run_dex_2026-11-26/treble_limit_epochs.json.gz"
    ));
    assert_eq!(replay["selector"], registry["selector"]);
    let markets = replay["markets"].as_array().unwrap();
    let mut seen = std::collections::BTreeSet::new();
    let mut fully_sold = 0;
    let mut unavailable = 0;
    for market in markets {
        let pool = market["roles"]["pool"]["address"].as_str().unwrap();
        assert!(seen.insert(pool));
        assert!(
            registry["markets"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["pool"] == pool && m["indexer_id"] == "treble-v4")
        );
        for leg in market["cases"].as_array().unwrap() {
            if leg["stage"] == "quote_buy" {
                assert_eq!(pool, "0x7fa82d3060316654c681cb941995c7d6aa29ef3b");
                assert_eq!(leg["reason"], "incomplete exact input");
                assert_eq!(number(&market["liquidity"]), U256::ZERO);
                unavailable += 1;
                continue;
            }
            let r = &leg["results"];
            assert_eq!(
                word(r, "before_in") - word(r, "buy_in"),
                number(&leg["amount"])
            );
            assert_eq!(
                word(r, "buy_out") - word(r, "before_out"),
                number(&leg["bought"])
            );
            assert_eq!(
                word(r, "buy_out") - word(r, "sell_in"),
                number(&leg["bought"])
            );
            assert_eq!(
                word(r, "sell_out") - word(r, "buy_in"),
                number(&leg["sold"])
            );
            assert_eq!(r["above_buy"]["status"], "0x0");
            assert_eq!(r["above_sell"]["status"], "0x0");
            fully_sold += 1;
        }
    }
    assert_eq!((seen.len(), fully_sold, unavailable), (6, 10, 2));
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
            if id == "treble-v4" {
                let topic = format!(
                    "{:#x}",
                    keccak256("SwapFee(address,uint24,uint24)".as_bytes())
                );
                for side in ["buy", "sell"] {
                    let logs = r[side]["logs"].as_array().unwrap();
                    let log = logs
                        .iter()
                        .find(|l| {
                            l["address"] == data["roles"]["pool"]["address"]
                                && l["topics"][0] == topic
                        })
                        .unwrap();
                    let data = log["data"].as_str().unwrap();
                    assert_eq!(
                        U256::from_str_radix(&data[2..66], 16).unwrap(),
                        U256::from(201)
                    );
                    assert_eq!(U256::from_str_radix(&data[66..], 16).unwrap(), U256::ZERO);
                }
            }
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

#[test]
fn treble_security_and_limit_order_roles_match_the_profile() {
    let raw = include_bytes!("data/first_run_dex_2026-11-26/treble_modules_base.json.gz");
    let modules: Value = serde_json::from_reader(GzDecoder::new(raw.as_slice())).unwrap();
    let cfg: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let f = &cfg
        .iter()
        .find(|r| r["id"] == "treble-v4" && r["chain_id"] == 8453)
        .unwrap()["settings"];
    let d = load("treble-v4");
    assert_eq!(f["dex.kind"], "algebra_integral_treble");
    assert_eq!(d["selector"], modules["selector"]);
    assert_eq!(d["global_state"][3], 215);
    assert_eq!(d["global_state"][2], 500);
    assert_eq!(
        f["dex.plugin_runtime_hashes"][0],
        d["roles"]["plugin_factory"]["keccak"]
    );
    for (i, k) in [(1, "limit_code"), (2, "security_code")] {
        let code = modules[k]
            .as_str()
            .unwrap()
            .parse::<alloy_primitives::Bytes>()
            .unwrap();
        assert_eq!(
            f["dex.plugin_runtime_hashes"][i],
            format!("{:#x}", keccak256(code))
        );
    }
    let responses = modules["responses"].as_array().unwrap();
    for (index, role) in [
        (0, "factory"),
        (1, "deployer"),
        (2, "plugin_factory"),
        (3, "factory"),
    ] {
        let raw = responses[index]["result"].as_str().unwrap();
        assert_eq!(
            &raw[26..],
            &d["roles"][role]["address"].as_str().unwrap()[2..]
        );
    }
    assert_eq!(
        U256::from_str_radix(&responses[4]["result"].as_str().unwrap()[2..], 16).unwrap(),
        U256::ZERO
    );
}
