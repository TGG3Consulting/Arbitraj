//! Старый формат карточек построен по схеме предыдущих версий. Ордер, хеши и блоки взяты из архивов.
use super::*;
use alloy_primitives::B256;
use flate2::read::GzDecoder;
use serde_json::{Value, json};

fn archive(bytes: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(bytes)).unwrap()
}

fn old(mode: RouteMode, label: &str, block: u64, executable: bool) -> Value {
    json!({"mode":mode, "legs":[], "block":block, "gas_units":0, "executable":executable, "exec_note":label})
}

#[test]
fn old_sources_migrate_once_without_routing_by_new_notes() {
    let rain = archive(include_bytes!("../../../tests/data/first_run_dex_2026-11-26/raindex_base_protected_wallets.json.gz"));
    let action = rain["pairs"].as_array().unwrap().iter()
        .flat_map(|p| p["cases"].as_array().unwrap())
        .flat_map(|c| c.as_object().unwrap().values())
        .find(|s| s["stage"] == "paid").unwrap();
    // Отпечаток старого билета не записан. Для проверки формата берём реальный идентификатор ордера.
    let rain_id: B256 = serde_json::from_value(action["order"].clone()).unwrap();
    let rain_block = rain["height"].as_u64().unwrap();
    let pcs = archive(include_bytes!("../../../tests/data/first_run_dex_2026-11-26/pancakex_base_history.json.gz"));
    let fill = &pcs["history"][0];
    assert_eq!(fill["transaction"]["hash"], fill["receipt"]["transactionHash"]);
    let pcs_id: B256 = serde_json::from_value(fill["order"]["hash"].clone()).unwrap();
    let pcs_block = u64::from_str_radix(fill["receipt"]["blockNumber"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
    use RouteMode::*;
    for (original, typed, label, height, executable) in [
        (Rfq(rain_id), Raindex(Some(rain_id)), "Raindex", rain_block, true),
        (RfqIndicative, Raindex(None), "Raindex", rain_block, false),
        (Orderbook(rain_id), RaindexSettlement(Some(rain_id)), "Raindex", rain_block, true),
        (OrderbookIndicative, RaindexSettlement(None), "Raindex", rain_block, false),
        (Relay(pcs_id), PancakeX(Some(pcs_id)), "PancakeSwap X", pcs_block, true),
        (RelayIndicative, PancakeX(None), "PancakeSwap X", pcs_block, false),
    ] {
        let mut route: DexRoute = serde_json::from_value(old(original, label, height, executable)).unwrap();
        assert_eq!(route.mode, typed);
        route.exec_note = Some("Другой текст карточки".to_owned());
        let reload = |r: &DexRoute| serde_json::from_value::<DexRoute>(serde_json::to_value(r).unwrap()).unwrap();
        assert_eq!(reload(&route), route);
        route.mode = original;
        route.exec_note = Some(label.into());
        assert_eq!(reload(&route).mode, original);
        let inconsistent: DexRoute = serde_json::from_value(old(original, label, height, !executable)).unwrap();
        assert_eq!(inconsistent.mode, original);
        assert!(serde_json::from_value::<DexRoute>(old(typed, label, height, !executable)).is_err());
    }
}

#[test]
fn recorded_bsc_direct_route_keeps_its_wire_and_physical_pool() {
    let quotes: Value = serde_json::from_str(include_str!("../../../tests/data/bsc_units_2026_11_24/signal_quotes.json")).unwrap();
    let saved = &quotes["sell_quote"]["route"];
    let route: DexRoute = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(route.mode, RouteMode::Direct);
    assert_eq!(route.block, 135478487);
    assert_eq!(serde_json::to_value(route).unwrap(), *saved);
    let mut other = saved.clone();
    other["mode"] = json!({"Relay": B256::ZERO});
    other["exec_note"] = json!("PancakeSwap X");
    assert_eq!(serde_json::from_value::<DexRoute>(other).unwrap().mode, RouteMode::Relay(B256::ZERO));
}
