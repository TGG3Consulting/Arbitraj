//! Файл: tests/core_transfer_net_430.rs
//! Что делает: решение 430 (шаг 2 двух сетей) — своё имя сети площадки для сети перевода (core/transfers.rs own_name):
//! сеть по умолчанию — venue.bsc_network площадки, как раньше (биржа BingX — BEP20, имя из её записанного справочника
//! tests/data/bingx_2026-10-24/config_getall.json); другая сеть — у биржи своего имени нет (имя сети по сети придёт из
//! её справочника, шаг 3), у площадки DEX этой сети — её venue.bsc_network.
//! Ядро — t198_common (SQLite во временной папке; bingx, mexc — биржи, bsc_dex — DEX).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::transfers, arb::settings. Моков нет.

mod t198_common;

use serde_json::{Value, json};

use arb::core::transfers::own_name;
use arb::core::types::{ChainId, VenueId};
use arb::gateway::contract::VenueKind;
use arb::settings::scope::Scope;

/// Площадка по id. Ошибок нет.
fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Задаёт настройку площадки. Паника — запись не прошла.
async fn set(s: &arb::settings::store::Store, name: &str, venue: &str, val: &str) {
    s.set(name, Scope::Venue(venue.into()), Some(json!(val)), "тест")
        .await
        .unwrap();
}

/// Имена сетей USDT в записанном справочнике BingX. Паника — файла нет.
fn bingx_usdt_networks() -> Vec<String> {
    let raw = std::fs::read_to_string("tests/data/bingx_2026-10-24/config_getall.json").unwrap();
    let data: Value = serde_json::from_str(&raw).unwrap();
    data["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["coin"] == "USDT")
        .flat_map(|c| c["networkList"].as_array().cloned().unwrap_or_default())
        .filter_map(|n| n["network"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn own_network_name_by_chain() {
    let nets = bingx_usdt_networks();
    assert!(nets.iter().any(|n| n == "BEP20") && nets.iter().any(|n| n == "ARBITRUM"));
    let core = t198_common::start("transfer-net-430").await;
    let s = core.inner.deps.settings.clone();
    set(&s, "venue.bsc_network", "bingx", "BEP20").await;
    set(&s, "venue.bsc_network", "bsc_dex", "BEP20").await;
    let arb = ChainId("arbitrum".into());
    // Сеть по умолчанию — как раньше.
    assert_eq!(own_name(&core, &v("bingx"), None).as_deref(), Some("BEP20"));
    assert_eq!(
        own_name(&core, &v("bsc_dex"), None).as_deref(),
        Some("BEP20")
    );
    // Другая сеть: у биржи имени BSC не даём.
    assert_eq!(own_name(&core, &v("bingx"), Some(&arb)), None);
    // Площадка DEX сети Arbitrum — своё имя.
    core.inner
        .kinds
        .write()
        .unwrap()
        .push((v("arb_dex"), VenueKind::Dex));
    set(&s, "venue.bsc_network", "arb_dex", "ARBITRUM").await;
    assert_eq!(
        own_name(&core, &v("arb_dex"), Some(&arb)).as_deref(),
        Some("ARBITRUM")
    );
}
