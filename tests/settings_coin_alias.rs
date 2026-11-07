//! Файл: tests/settings_coin_alias.rs
//! Что делает: решение 211, ревью В8 и В9 на настоящей SQLite во временной папке. В9: значения области «монета» по
//! адресу нет — берётся старое по тикеру монеты (settings/coin_alias.rs, Store::resolve); значение по адресу главнее.
//! В8: ручной чёрный список — явное значение по адресу (да ИЛИ нет) главнее старой отметки по тикеру (core/blacklist.rs).
//! Адрес CAKE в BSC — из записанного справочника сетей BingX (tests/data/bingx_2026-10-24/config_getall.json).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::settings, arb::core::blacklist, arb::storage. Моков нет.

use std::collections::HashMap;

use alloy_primitives::{Address, address};
use serde_json::json;
use tokio::sync::broadcast;

use arb::core::blacklist;
use arb::core::types::Asset;
use arb::settings::read::At;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;

const CAKE: Address = address!("0x0e09fabb73bd3ade0a17ecc321fd13a19e81ce82");

/// Новая база во временной папке.
async fn fresh(name: &str) -> Store {
    let dir = std::env::temp_dir().join(format!("arb-coin-alias-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (tx, _) = broadcast::channel(16);
    let (jtx, _) = broadcast::channel(16);
    Store::load(db.clone(), tx, Journal::new(db, jtx))
        .await
        .unwrap()
}

#[tokio::test]
async fn v9_coin_value_falls_back_to_ticker() {
    let s = fresh("v9").await;
    let id = format!("{CAKE:#x}");
    let name = "transfer.credit_amount_tolerance_pct";
    s.set(name, Scope::Coin("CAKE".into()), Some(json!("0.5")), "тест")
        .await
        .unwrap();
    let at = At {
        venue: None,
        coin: Some(&id),
    };
    // Ядро ещё не сообщило тикеры монеты — по адресу не задано.
    assert!(s.decimal(name, at).is_err());
    s.coin_aliases()
        .set(HashMap::from([(id.clone(), vec!["CAKE".to_string()])]));
    assert_eq!(s.decimal(name, at).unwrap().to_string(), "0.5");
    // Значение по адресу главнее старого по тикеру.
    s.set(name, Scope::Coin(id.clone()), Some(json!("0.7")), "тест")
        .await
        .unwrap();
    assert_eq!(s.decimal(name, at).unwrap().to_string(), "0.7");
}

#[tokio::test]
async fn v8_explicit_address_value_wins_over_ticker_block() {
    let s = fresh("v8").await;
    let id = format!("{CAKE:#x}");
    let cake = Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    };
    s.set(
        "coin.blocked",
        Scope::Coin("CAKE".into()),
        Some(json!(true)),
        "тест",
    )
    .await
    .unwrap();
    // Старая отметка по тикеру держит монету, пока по адресу ничего не задано.
    assert_eq!(blacklist::manual(&s, &cake, &["CAKE"]), Some(None));
    // Человек снял блок по адресу (нет) — старая отметка по тикеру больше не держит.
    s.set(
        "coin.blocked",
        Scope::Coin(id.clone()),
        Some(json!(false)),
        "тест",
    )
    .await
    .unwrap();
    assert_eq!(blacklist::manual(&s, &cake, &["CAKE"]), None);
    // Да по адресу — блок с заметкой адреса.
    s.set(
        "coin.blocked",
        Scope::Coin(id.clone()),
        Some(json!(true)),
        "тест",
    )
    .await
    .unwrap();
    s.set(
        "coin.blocked_note",
        Scope::Coin(id),
        Some(json!("ручная")),
        "тест",
    )
    .await
    .unwrap();
    assert_eq!(
        blacklist::manual(&s, &cake, &["CAKE"]),
        Some(Some("ручная".into()))
    );
}
