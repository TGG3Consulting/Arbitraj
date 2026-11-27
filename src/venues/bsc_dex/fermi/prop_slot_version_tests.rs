//! Проверка допуска для автоматической торговли PropAMM. Офлайн-EVM сверяет те же ответы
//! slotVersion() с архивным байткодом обоих поколений для Kaliber и Taurus.
use super::{slot, supports_single_use};
use alloy::primitives::Bytes;
use alloy::sol_types::SolCall;
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn admits_only_the_single_use_slot_getter_result() {
    let archive: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/prop_slot_runtime_v1.json.gz")[..],
    )).unwrap();
    for (generation, allowed) in [("v1", false), ("v2", true)] {
        let raw: Bytes = archive["expectedSlotVersionReturns"][generation].as_str().unwrap().parse().unwrap();
        let version = slot::slotVersionCall::abi_decode_returns(&raw).unwrap();
        assert_eq!(supports_single_use(version), allowed, "{generation}");
    }
}
