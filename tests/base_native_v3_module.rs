//! Адреса, выплаты и защитные отказы нативных V3-роутеров Base в архивной симуляции GitHub.
use alloy_primitives::{B256, U256};
use arb::venues::bsc_dex::pools::{direct_module, families};
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
    assert!(direct_module::accepts(8453, "base_native_v3", hash));
    for chain in [1, 56, 42161] {
        assert!(!direct_module::accepts(chain, "base_native_v3", hash));
    }
}
