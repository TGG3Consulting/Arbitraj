//! Реестр CREATE2 2THICK использует шаг тика, а стоимость обмена определяется текущей ставкой пула.
use super::super::{exec_kind::ExecKind, families, per_block};
use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn spacing_is_the_registry_key_and_current_fee_is_observed_without_an_event() {
    let raw =
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/wagmi_thick_base.json.gz");
    let snapshot: Value = serde_json::from_reader(GzDecoder::new(raw.as_slice())).unwrap();
    let family = families::make("spacing-market", "registered_spacing_v3").unwrap();
    assert!(family.variant.exact_router());
    assert_eq!(family.executor_kind, Some(ExecKind::V3));
    assert!(per_block::light(family));
    let data = &snapshot["2thick"];
    let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let factory = addr(&data["roles"]["factory"]["address"]);
    let init = data["init"].as_str().unwrap().parse::<B256>().unwrap();
    let records = data["markets"].as_array().unwrap();
    assert_eq!(records.len(), 47);
    let mut changed = 0;
    for m in records {
        let (a, b) = (addr(&m["tokens"][0]), addr(&m["tokens"][1]));
        let pool = addr(&m["pool"]);
        let spacing =
            I24::try_from(i32::try_from(m["spacing"].as_i64().unwrap()).unwrap()).unwrap();
        assert_eq!(m["key"], m["spacing"]);
        assert_eq!(spacing_address(factory, (a, b), spacing, init), pool);
        let fee = m["current_fee"].as_u64().unwrap();
        assert_ne!(address(factory, (a, b), U24::from(fee), init), pool);
        assert_eq!(m["initial_fee"], 10000);
        changed += usize::from(fee != 10000);
    }
    assert_eq!(changed, 11);
    let profiles: Value =
        serde_json::from_str(include_str!("../../../../config/first_run_dex.json")).unwrap();
    let settings = &profiles
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "2thick-base")
        .unwrap()["settings"];
    assert_eq!(settings["dex.v3_wallet_router_abi"], "router02_spacing");
    assert_eq!(settings["dex.pool_code_hashes"][0], data["init"]);
    let vvv = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/2thick_venice_base_wallets.json.gz"
    );
    let funded: Value = serde_json::from_reader(GzDecoder::new(vvv.as_slice())).unwrap();
    assert_eq!(funded["selector"], data["selector"]);
    let bindings = settings["dex.router_exact_tokens"].as_array().unwrap();
    for token in funded["market"]["tokens"].as_array().unwrap() {
        assert!(
            bindings
                .iter()
                .any(|row| row.as_str().unwrap().split_whitespace().next() == token.as_str())
        );
    }
    let cases = funded["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 8);
    for leg in cases.iter().filter(|c| c["wire"] == "router02") {
        let peer = cases
            .iter()
            .find(|c| c["wire"] == "v5" && c["input"] == leg["input"] && c["spent"] == leg["spent"])
            .unwrap();
        assert_eq!(peer["received"], leg["received"]);
        assert!(leg["gas"].as_u64().unwrap() > peer["gas"].as_u64().unwrap());
    }
}

#[test]
fn registered_slipstream_clones_use_observed_pool_fee_and_the_existing_v5() {
    let raw=include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/virtus_cl_base.json.gz");
    let book:Value=serde_json::from_reader(GzDecoder::new(raw.as_slice())).unwrap();
    let f=families::make("custom", "registered_slipstream").unwrap();
    assert!(f.variant.checked_slipstream_fee() && f.variant.exact_router() && per_block::light(f));
    assert!(!f.variant.origin_fee() && !f.variant.spacing_keyed());
    assert_eq!(f.executor_kind,Some(ExecKind::V3));
    let address=|v:&Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let markets=book["markets"].as_array().unwrap();
    let init=book["init"].as_str().unwrap().parse::<B256>().unwrap();
    let factory=address(&book["roles"]["factory"]["address"]);
    assert_eq!(markets.len(),9);
    for m in markets {
        let tokens=(address(&m["tokens"][0]),address(&m["tokens"][1]));
        let step=I24::try_from(m["spacing"].as_i64().unwrap()).unwrap();
        assert_eq!(spacing_address(factory,tokens,step,init),address(&m["pool"]));
    }
    assert_eq!(markets.iter().filter(|m|m["current_fee"]!=m["base_fee"]).count(),3);
}
