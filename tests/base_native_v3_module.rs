//! Адреса, выплаты и защитные отказы нативных V3-роутеров Base в архивной симуляции GitHub.
#[path = "common/first_run_profiles.rs"]
mod first_run_profiles;
use alloy_primitives::{B256, U256};
use arb::venues::bsc_dex::pools::families;
use serde_json::Value;

#[test]
fn archive_wallet_receipts_match_each_accepted_router_and_module() {
    let evidence: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/base_native_v3.json"
    ))
    .unwrap();
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let hash: B256 = evidence["module_runtime"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let amount = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let mut passed = 0;
    for (name, market) in evidence["markets"].as_object().unwrap() {
        let p = profiles
            .iter()
            .find(|p| p["network"] == "base" && p["id"] == name.as_str())
            .unwrap();
        assert_eq!(p["settings"]["dex.factory"], market["factory"]);
        assert_eq!(p["settings"]["dex.quoter"], market["quoter"]["address"]);
        let selector = if name == "throne-v3-base" {
            "quoteExactInputSingleV3"
        } else {
            "quoteExactInputSingle"
        };
        assert_eq!(p["settings"]["dex.v3_quoter_method"], selector);
        assert_eq!(market["quoter"]["deployer"], market["router"]["deployer"]);
        assert_eq!(market["quoter"]["callbacks"], market["router"]["callbacks"]);
        assert_eq!(
            p["verified_module"]["runtime_keccak"],
            evidence["module_runtime"]
        );
        let mut swaps = 0;
        for case in market["cases"].as_array().unwrap() {
            if case["buy"]["quote_error"].is_object() {
                assert_eq!(name, "omni-exchange-v3-base");
                assert!(case.get("sell").is_none());
                continue;
            }
            for side in ["buy", "sell"] {
                let row = &case[side];
                assert_eq!(amount(&row["spent"]), amount(&row["requested"]));
                assert_eq!(amount(&row["received"]), amount(&row["module"]["net"]));
                assert!(amount(&row["received"]) > U256::ZERO);
                assert!(
                    row["module"]["minimum_plus_one"].is_object()
                        && row["module"]["expired"].is_object()
                );
                swaps += 1;
            }
            assert_eq!(case["buy"]["received"], case["sell"]["spent"]);
            passed += 1;
        }
        assert_eq!(p["verified_module"]["full"].as_u64(), Some(swaps));
    }
    assert_eq!(passed, 7);
    let observer = families::make("omni-exchange-v3-base", "base_native_v3").unwrap();
    let active = families::make("omni-exchange-v3-base", "base_native_v3_module").unwrap();
    assert!(observer.graphable && !observer.tradable && observer.executor_kind.is_none());
    assert!(
        active.tradable && active.fork && active.direct_module() && active.executor_kind.is_none()
    );
    assert!(first_run_profiles::recommended(8453, "omni-exchange-v3-base").contains(&hash));
    for chain in [1, 56, 42161] {
        assert!(!first_run_profiles::recommended(chain, "omni-exchange-v3-base").contains(&hash));
    }
}
#[test]
fn configured_search_includes_every_created_pool_fee_on_the_archive_block() {
    use std::collections::BTreeSet;
    let directory: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/base_native_v3_directory.json"
    ))
    .unwrap();
    let wallet: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/base_native_v3.json"
    ))
    .unwrap();
    assert_eq!(directory["selector"], wallet["selector"]);
    assert_eq!(directory["state_root"], wallet["state_root"]);
    for (id, expected) in [
        ("9mm-v3-base", 896),
        ("throne-v3-base", 24),
        ("omni-exchange-v3-base", 50),
    ] {
        let market = &directory["markets"][id];
        let profile = first_run_profiles::named(8453, id).unwrap();
        let settings = &profile["settings"];
        assert_eq!(market["factory"], settings["dex.factory"]);
        let pools = market["pools"].as_array().unwrap();
        assert_eq!(pools.len(), expected);
        assert_eq!(market["nonce"].as_u64(), Some(expected as u64 + 1));
        let used: BTreeSet<_> = pools.iter().map(|p| p["fee"].as_u64().unwrap()).collect();
        let configured: BTreeSet<_> = settings["dex.v3_fee_tiers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().parse::<u64>().unwrap())
            .collect();
        assert!(
            used.is_subset(&configured),
            "{id} misses a created fee tier"
        );
        let unique: BTreeSet<_> = pools.iter().map(|p| p["pool"].as_str().unwrap()).collect();
        assert_eq!(unique.len(), expected);
    }
}

#[test]
fn omni_both_funded_directions_reconcile_at_the_larger_gas_limit() {
    let evidence: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/base_native_v3.json"
    ))
    .unwrap();
    let market = &evidence["markets"]["omni-exchange-v3-base"];
    let funding = &market["reverse_funding"];
    assert_ne!(funding["pool"], market["pool"]);
    assert_eq!(funding["spent_usdc"], "12000000");
    assert_eq!(funding["received_frxusd"], "11988715522199938127");
    for (field, input, unit, passed) in [
        ("extended_cash", 0, 1_000_000u128, 2),
        ("extended_frxusd", 1, 1_000_000_000_000_000_000u128, 3),
    ] {
        let rows = market[field].as_array().unwrap();
        assert_eq!(rows.len(), 4);
        for (i, (case, amount)) in rows
            .iter()
            .zip([unit / 20, unit / 10, unit, 10 * unit])
            .enumerate()
        {
            let buy = &case["buy"];
            assert_eq!(buy["input"], market["tokens"][input]);
            assert_eq!(buy["requested"], amount.to_string());
            assert_eq!(buy["gas_limit"], 16_777_216);
            if i >= passed {
                assert_eq!(
                    buy["quote_error"]["message"],
                    "execution reverted: Unexpected error"
                );
                assert!(case.get("sell").is_none());
                continue;
            }
            assert_eq!(case["sell"]["requested"], buy["received"]);
            assert_eq!(case["sell"]["input"], buy["output"]);
            assert_eq!(case["sell"]["output"], buy["input"]);
            for leg in [buy, &case["sell"]] {
                assert_eq!(leg["requested"], leg["spent"]);
                assert_eq!(leg["quoted"], leg["received"]);
                assert_eq!(leg["received"], leg["module"]["net"]);
                assert!(leg["received"].as_str().unwrap().parse::<u128>().unwrap() > 0);
                for route in [leg, &leg["module"]] {
                    assert_eq!(
                        route["minimum_plus_one"]["message"],
                        "execution reverted: Too little received"
                    );
                    assert_eq!(route["expired"]["code"], 3);
                }
            }
        }
    }
    assert_eq!(market["extended_frxusd"][2]["buy"]["received"], "999945");
    assert_eq!(
        market["extended_frxusd"][2]["sell"]["received"],
        "999799709272636829"
    );
}
