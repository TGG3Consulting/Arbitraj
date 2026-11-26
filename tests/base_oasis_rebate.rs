use alloy_primitives::U256;
use arb::venues::bsc_dex::math::v2;
use serde_json::Value;

fn raw(value: &Value) -> U256 {
    value.as_str().unwrap().parse().unwrap()
}

#[test]
fn new_module_preserves_archive_and_falls_back_only_outside_optional_fee_domain() {
    let before: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/oasisswap_v5_base.json"
    ))
    .unwrap();
    let current: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/oasisswap_v5_capped_base.json"
    ))
    .unwrap();
    let probe: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/oasisswap_rebate_base.json"
    ))
    .unwrap();
    let cfg: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let allowed = &cfg.iter().find(|p| p["id"] == "oasisswap-base").unwrap()["settings"]["dex.module_runtime_hashes"];
    for snapshot in [&before, &current] {
        assert!(
            allowed
                .as_array()
                .unwrap()
                .contains(&snapshot["module_runtime_hash"])
        );
        assert_eq!(snapshot["selector"], probe["selector"]);
        assert_eq!(snapshot["state_root"], probe["state_root"]);
    }
    assert_ne!(
        current["module_runtime_hash"],
        before["module_runtime_hash"]
    );
    assert_eq!(current["v5_runtime_hash"], before["v5_runtime_hash"]);
    let old = before["cases"].as_array().unwrap();
    let new = current["cases"].as_array().unwrap();
    assert_eq!(old.len(), new.len());
    for (old, new) in old.iter().zip(new) {
        for k in ["pool", "size", "asset", "token"] {
            assert_eq!(old[k], new[k]);
        }
        for side in ["buy", "sell"] {
            for (kind, receipt) in old[side].as_object().unwrap() {
                for k in [
                    "status",
                    "spent",
                    "received",
                    "floor",
                    "native_quote",
                    "fee_cache",
                ] {
                    assert_eq!(receipt[k], new[side][kind][k]);
                }
            }
        }
    }
    assert_ne!(probe["payer"], probe["recipient_in_adapter"]);
    let dx = raw(&probe["amount"]);
    let reserve = raw(&probe["reserve"]);
    let cases = probe["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    for (r, value) in cases.iter().zip([0u64, 1000, 10000, 10001]) {
        let factor = if value > 10000 { 10000 } else { 10000 - value };
        let rate = 100 * factor / 10000;
        let out = v2::amount_out(
            dx,
            reserve,
            reserve,
            (U256::from(10000 - rate), U256::from(10000)),
        )
        .unwrap();
        assert_eq!(r["rebate"], value);
        assert_eq!(r["recipient_rebate"], 0);
        assert_eq!(r["fee"], rate);
        assert_eq!(raw(&r["received"]), out);
        assert_eq!(
            raw(&r["fee_cache_delta"]),
            dx * U256::from(rate) / U256::from(20000)
        );
        assert_eq!(r["min_plus_one"], "0x0");
        assert_eq!(r["wrong_factory"], "0x0");
        if value > 10000 {
            assert!(r["native_discounted"].is_null());
            assert_eq!(raw(&r["nominal"]), out);
        } else {
            assert_eq!(raw(&r["native_discounted"]), out);
        }
    }
}
