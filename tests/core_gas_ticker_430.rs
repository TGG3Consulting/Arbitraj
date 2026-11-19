//! Файл: tests/core_gas_ticker_430.rs
//! Что делает: решение 430 (шаг 2 двух сетей) — газ по сети (core/gas_ticker.rs): одна площадка DEX — её тикер для
//! монеты без сети и для биржи (как раньше); две площадки DEX в разных сетях — газ сети монеты (of_asset), у биржи —
//! газ сети сделки (for_venue_in), бирже — стаканы газа всех сетей (all).
//! Ядро — t198_common (SQLite во временной папке; bingx, mexc — биржи, bsc_dex — DEX). Монета — направление сделки из
//! выгрузки живого бота (tests/data/sigma_windows_2026-11-16/deals_sell_on_cex.jsonl).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::gas_ticker, arb::settings. Моков нет.

mod t198_common;

use arb::core::gas_ticker;
use arb::core::types::{ChainId, Direction, VenueId};
use arb::gateway::contract::VenueKind;
use arb::settings::scope::Scope;
use serde_json::{Value, json};

const DEALS: &str = "tests/data/sigma_windows_2026-11-16/deals_sell_on_cex.jsonl";

/// Задаёт настройку. Паника — запись не прошла.
async fn set(s: &arb::settings::store::Store, name: &str, scope: Scope, v: &str) {
    s.set(name, scope, Some(json!(v)), "тест").await.unwrap();
}

/// Первое направление сделки из выгрузки живого бота. Паника — файла нет.
fn recorded() -> Direction {
    let raw = std::fs::read_to_string(DEALS).unwrap();
    let line = raw.lines().find(|l| !l.trim().is_empty()).unwrap();
    serde_json::from_value(serde_json::from_str::<Value>(line).unwrap()["direction"].clone())
        .unwrap()
}

#[tokio::test]
async fn gas_by_chain() {
    let core = t198_common::start("gas-ticker-430").await;
    let s = core.inner.deps.settings.clone();
    let mexc = VenueId("mexc".into());
    let bsc = recorded().asset;
    set(&s, "dex.gas_ticker", Scope::Venue("bsc_dex".into()), "BNB").await;
    // Одна площадка DEX — без dex.chain, как раньше.
    assert_eq!(gas_ticker::of_asset(&core, &bsc).unwrap(), "BNB");
    assert_eq!(gas_ticker::for_venue_in(&core, &mexc, None).unwrap(), "BNB");
    assert_eq!(gas_ticker::all(&core), vec!["BNB".to_string()]);
    // Вторая площадка DEX другой сети.
    core.inner
        .kinds
        .write()
        .unwrap()
        .push((VenueId("arb_dex".into()), VenueKind::Dex));
    set(&s, "dex.gas_ticker", Scope::Venue("arb_dex".into()), "ETH").await;
    set(&s, "dex.chain", Scope::Venue("bsc_dex".into()), "bsc").await;
    set(&s, "dex.chain", Scope::Venue("arb_dex".into()), "arbitrum").await;
    let e = gas_ticker::of_asset(&core, &bsc).unwrap_err();
    assert!(e.contains("dex.default_chain"), "{e}");
    set(&s, "dex.default_chain", Scope::Global, "bsc").await;
    assert_eq!(gas_ticker::of_asset(&core, &bsc).unwrap(), "BNB");
    let mut arb = bsc.clone();
    arb.chain = Some(ChainId("arbitrum".into()));
    assert_eq!(gas_ticker::of_asset(&core, &arb).unwrap(), "ETH");
    assert_eq!(
        gas_ticker::for_venue_in(&core, &mexc, arb.chain.as_ref()).unwrap(),
        "ETH",
        "биржа — газ сети сделки"
    );
    assert_eq!(gas_ticker::for_venue_in(&core, &mexc, None).unwrap(), "BNB");
    assert_eq!(gas_ticker::all(&core), vec!["BNB".to_string(), "ETH".to_string()]);
}
