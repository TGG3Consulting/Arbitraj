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
