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
}
