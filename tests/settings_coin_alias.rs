//! Файл: tests/settings_coin_alias.rs
//! Что делает: решение 211, ревью В8 и В9 на настоящей SQLite во временной папке. В9 (второе ревью, п.3): значения
//! области «монета» по адресу нет — из старых по тикеру применяется только coin.blocked = да, прочие (и «нет») не
//! применяются — действует площадка/вся система (settings/coin_alias.rs, Store::resolve); значение по адресу главнее.
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
async fn v9_ticker_value_not_applied_to_address_coin() {
    let s = fresh("v9").await;
    let id = format!("{CAKE:#x}");
    let name = "transfer.credit_amount_tolerance_pct";
    s.set(name, Scope::Coin("CAKE".into()), Some(json!("0.5")), "тест")
        .await
        .unwrap();
    s.set(
        name,
        Scope::Venue("bingx".into()),
        Some(json!("0.3")),
        "тест",
    )
    .await
    .unwrap();
    s.coin_aliases()
        .set(HashMap::from([(id.clone(), vec!["CAKE".to_string()])]));
    let at = At {
        venue: Some("bingx"),
        coin: Some(&id),
    };
    // Второе ревью 211, п.3: значение по тикеру неоднозначно — не применяется, действует уровень площадки.
    assert_eq!(s.decimal(name, at).unwrap().to_string(), "0.3");
    let bare = At {
        venue: None,
        coin: Some(&id),
    };
    assert!(s.decimal(name, bare).is_err());
    // Значение по адресу главнее всего.
    s.set(name, Scope::Coin(id.clone()), Some(json!("0.7")), "тест")
        .await
        .unwrap();
    assert_eq!(s.decimal(name, at).unwrap().to_string(), "0.7");
}

#[tokio::test]
async fn v9_only_blocked_true_falls_back_to_ticker() {
    let s = fresh("v9b").await;
    let id = format!("{CAKE:#x}");
    s.coin_aliases()
        .set(HashMap::from([(id.clone(), vec!["CAKE".to_string()])]));
    let at = At {
        venue: None,
        coin: Some(&id),
    };
    // Нет по тикеру — не применяется (по адресу ничего не задано).
    s.set(
        "coin.blocked",
        Scope::Coin("CAKE".into()),
        Some(json!(false)),
        "тест",
    )
    .await
    .unwrap();
    assert!(s.flag("coin.blocked", at).is_err());
    // Да по тикеру — применяется (безопасно: монета не торгуется).
    s.set(
        "coin.blocked",
        Scope::Coin("CAKE".into()),
        Some(json!(true)),
        "тест",
    )
    .await
    .unwrap();
    assert!(s.flag("coin.blocked", at).unwrap());
    // Нет по адресу — главнее старого «да» по тикеру.
    s.set(
        "coin.blocked",
        Scope::Coin(id.clone()),
        Some(json!(false)),
        "тест",
    )
    .await
    .unwrap();
    assert!(!s.flag("coin.blocked", at).unwrap());
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
