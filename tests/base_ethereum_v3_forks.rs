use alloy_primitives::{U256, keccak256};
use arb::venues::bsc_dex::quote::virtual_quoter;
use serde_json::Value;

/// Проверяет происхождение кода допущенных профилей и исполнение граничного minOut на обоих плечах.
#[test]
fn source_matched_v3_forks_settle_exact_amounts() {
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let virtual_hash = format!("{:#x}", keccak256(virtual_quoter::runtime().unwrap()));
    for (network, fixture) in [
        (
            "base",
            include_str!("data/first_run_dex_2026-11-26/verified_v3_base.json"),
        ),
        (
            "ethereum",
            include_str!("data/first_run_dex_2026-11-26/verified_v3_ethereum.json"),
        ),
    ] {
        let capture: Value = serde_json::from_str(fixture).unwrap();
        for (id, entry) in capture.as_object().unwrap() {
            let market = &entry["pool"];
            let proof = &entry["deployment"];
            let execution = &market["execution"];
            assert_eq!(execution["virtual_quoter_keccak"], virtual_hash);
            let rows = execution["pools"][0]["cases"].as_array().unwrap();
            assert_eq!(rows.len(), 2);
            for row in rows {
                let raw = |key: &str| row[key].as_str().unwrap().parse::<U256>().unwrap();
                assert_eq!(
                    raw("cash_before") - raw("input") + raw("sold"),
                    raw("cash_after")
                );
                assert!(raw("bought") > U256::ZERO);
                assert_eq!(row["min_out_plus_one_buy_reverted"], true);
                assert_eq!(row["min_out_plus_one_sell_reverted"], true);
            }
            if proof["admitted"] != true {
                continue;
            }
            let profile = profiles
                .iter()
                .find(|p| p["network"] == network && p["id"].as_str() == Some(id.as_str()))
                .unwrap();
            assert_eq!(
                profile["settings"]["dex.factory"]
                    .as_str()
                    .unwrap()
                    .to_ascii_lowercase(),
                market["factory"].as_str().unwrap().to_ascii_lowercase()
            );
            assert_eq!(
                proof["factory"]["digest"],
                proof["reference_factory"]["digest"]
            );
            if proof.get("deployer").is_some() {
                assert_eq!(
                    proof["deployer"]["digest"],
                    proof["reference_deployer"]["digest"]
                );
            }
            assert!(proof["fee_tiers"].get(market["fee"].to_string()).is_some());
        }
    }
}

#[test]
fn rubicon_registered_pools_reproduce_the_verified_creation_address() {
    use alloy::sol_types::SolValue;
    use alloy_primitives::{Address, B256};
    let registry: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/rubicon_clmm_ethereum.json"
    ))
    .unwrap();
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let profile = &profiles
        .iter()
        .find(|p| p["id"] == "rubicon-clmm-ethereum")
        .unwrap()["settings"];
    let factory: Address = registry["factory"].as_str().unwrap().parse().unwrap();
    let init: B256 = registry["creation_hash"].as_str().unwrap().parse().unwrap();
    assert_eq!(profile["dex.factory"], registry["factory"]);
    assert_eq!(profile["dex.tick_lens"], registry["tick_lens"]);
    assert_eq!(profile["dex.quoter_virtual"], true);
    let enabled = profile["dex.v3_fee_tiers"].as_array().unwrap();
    assert_eq!(enabled.len(), registry["tiers"].as_object().unwrap().len());
    for key in registry["tiers"].as_object().unwrap().keys() {
        assert!(enabled.iter().any(|v| v.as_str() == Some(key.as_str())));
    }
    let pools = registry["pools"].as_array().unwrap();
    assert_eq!(pools.len(), 11);
    for pool in pools {
        let address = |index: usize| {
            pool["tokens"][index]
                .as_str()
                .unwrap()
                .parse::<Address>()
                .unwrap()
        };
        let expected: Address = pool["address"].as_str().unwrap().parse().unwrap();
        let fee = U256::from(pool["fee"].as_u64().unwrap());
        let salt = keccak256((address(0), address(1), fee).abi_encode());
        assert_eq!(factory.create2(salt, init), expected);
    }
    let replay: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/verified_v3_ethereum.json"
    ))
    .unwrap();
    let market = &replay["rubicon-clmm-ethereum"]["pool"];
    assert_eq!(registry["selector"]["blockHash"], market["hash"]);
    assert_eq!(registry["lens_sample"]["pool"], market["pool"]);
    assert!(
        pools
            .iter()
            .any(|p| p["address"] == market["pool"] && p["liquidity"] != "0")
    );
}

#[test]
fn rubicon_base_replay_uses_the_profiled_factory_and_public_book() {
    let registry: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/rubicon_clmm_base.json"
    ))
    .unwrap();
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let settings = &profiles
        .iter()
        .find(|p| p["id"] == "rubicon-clmm-base")
        .unwrap()["settings"];
    assert_eq!(settings["dex.factory"], registry["factory"]);
    assert_eq!(settings["dex.tick_lens"], registry["tick_lens"]);
    assert_eq!(settings["dex.quoter_virtual"], true);
    let execution = &registry["execution"];
    assert_eq!(registry["selector"]["blockHash"], execution["hash"]);
    assert_eq!(registry["registry_pairs"], 47);
    assert_eq!(registry["pools"].as_array().unwrap().len(), 32);
    let witnessed = execution["pools"].as_array().unwrap();
    assert_eq!(witnessed.len(), 3);
    for market in witnessed {
        assert!(
            registry["pools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["pool"] == market["pool"])
        );
        let trades = market["cases"].as_array().unwrap();
        assert_eq!(trades.len(), 2);
        for trade in trades {
            let uint = |k: &str| trade[k].as_str().unwrap().parse::<U256>().unwrap();
            assert_eq!(
                uint("cash_after"),
                uint("cash_before") - uint("input") + uint("sold")
            );
            assert_eq!(uint("after_coin_executor"), U256::ZERO);
            assert_eq!(uint("after_cash_executor"), U256::ZERO);
            assert_eq!(trade["min_out_plus_one_buy_reverted"], true);
            assert_eq!(trade["min_out_plus_one_sell_reverted"], true);
        }
    }
}
