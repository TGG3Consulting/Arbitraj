//! Three attested factory generations and both strict monetary controls on actual archived calls.
use super::*;
use flate2::read::GzDecoder;
fn archive(raw: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(raw)).unwrap()
}
#[test]
fn canonical_registries_bind_distinct_amms_and_typed_ids() {
    let j = archive(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/llamma_ethereum_registry.json.gz"
    ));
    let mut seen = std::collections::HashSet::new();
    for group in ["oneway", "mint", "v2"] {
        let profiles = j["runtimes"][group]["classes"].as_array().unwrap();
        for m in j["markets"][group].as_array().unwrap() {
            assert_eq!(m["amm"], m["controllerAmm"]);
            assert_eq!(m["controller"], m["poolAdmin"]);
            let address = m["amm"].as_str().unwrap().parse().unwrap();
            let idx = m["index"].as_u64().unwrap();
            assert!(seen.insert(address));
            let id = key(idx, address);
            assert_eq!(decode_key(id), Some((idx, address)));
            let mut wrong = id.0;
            wrong[0] = 1;
            assert!(decode_key(B256::from(wrong)).is_none());
            assert_eq!(
                profiles
                    .iter()
                    .flat_map(|p| p["markets"].as_array().unwrap())
                    .filter(|p| p["amm"] == m["amm"])
                    .count(),
                1
            );
        }
    }
    assert_eq!(seen.len(), 65);
}
#[test]
fn actual_v5_refunds_and_measured_usdt_bridges() {
    let j = archive(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/llamma_ethereum_wallets.json.gz"
    ));
    let mut partial = 0;
    for r in j["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["mode"] == "v5" && r["funding"] == "WETH.deposit")
    {
        let fill = &r["result"];
        let num = |k: &str| fill[k].as_str().unwrap().parse::<U256>().unwrap();
        assert_eq!(num("spent") + num("returnedByExecutor"), num("cap"));
        assert!(num("spent") < num("cap"));
        assert_eq!(num("spent"), U256::from(16368086465633u64));
        partial += 1;
    }
    assert_eq!(partial, 2);
    let paths = j["usdt"].as_array().unwrap();
    assert_eq!(paths.len(), 4);
    for p in paths {
        assert_eq!(p["status"], "paid");
        assert_eq!(p["changeCrvUSD"], "0");
        assert_eq!(p["control"], json!(["0x0", "0x0", "0x1"]));
        assert!(p["steps"] == json!([4, 9]) || p["steps"] == json!([9, 4]));
    }
}
