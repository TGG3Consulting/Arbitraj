//! Файл: tests/core_coin_scope_migrate.rs
//! Что проверяет: решение 211 — перенос настроек области «монета» с тикера на адрес контракта на настоящем хранилище
//! настроек (SQLite): однозначный тикер переносится (значение по адресу, по тикеру — очищено, журнал изменений с
//! автором «миграция решения 211»); неоднозначный (несколько адресов) остаётся по тикеру; уже заданное по адресу не
//! перезаписывается; повторный запуск ничего не меняет. Ручной блок по неоднозначному тикеру продолжает действовать
//! (core/blacklist.rs manual). Убранные из каталога настройки (coin.ticker_alias*) в базе не мешают загрузке
//! хранилища и не переносятся.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::coin_scope_migrate, arb::core::blacklist, arb::settings, arb::storage.

use std::collections::{BTreeMap, BTreeSet};

use alloy_primitives::Address;
use serde_json::json;
use tokio::sync::broadcast;

use arb::core::blacklist;
use arb::core::coin_scope_migrate::{AUTHOR, apply, old_ids, plan};
use arb::core::types::Asset;
use arb::settings::scope::{Scope, coin_id};
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;

async fn store(tag: &str) -> Store {
    let d = std::env::temp_dir().join(format!("arb-coin-scope-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    Store::load(db.clone(), changes, Journal::new(db, jtx))
        .await
        .unwrap()
}

#[tokio::test]
async fn migrate_unambiguous_keep_ambiguous_idempotent() {
    let s = store("main").await;
    let (cake, b1, b2) = (
        Address::repeat_byte(1),
        Address::repeat_byte(3),
        Address::repeat_byte(4),
    );
    for (t, note) in [("CAKE", "скам"), ("BOMB", "два токена")] {
        let sc = Scope::Coin(t.into());
        s.set("coin.blocked", sc.clone(), Some(json!(true)), "тест")
            .await
            .unwrap();
        s.set("coin.blocked_note", sc, Some(json!(note)), "тест")
            .await
            .unwrap();
    }
    let mut by: BTreeMap<String, BTreeSet<Address>> = BTreeMap::new();
    by.entry("CAKE".into()).or_default().insert(cake);
    by.entry("BOMB".into()).or_default().extend([b1, b2]);
    let p = plan(&old_ids(&s), &by);
    let (moved, conflicts, failed) = apply(&s, &p).await;
    assert_eq!((moved, conflicts.len(), failed.len()), (2, 0, 0));
    let at = Scope::Coin(coin_id("CAKE", Some(&cake)));
    assert_eq!(s.get_exact("coin.blocked", &at), Some(json!(true)));
    assert_eq!(s.get_exact("coin.blocked_note", &at), Some(json!("скам")));
    assert_eq!(
        s.get_exact("coin.blocked", &Scope::Coin("CAKE".into())),
        None
    );
    // Неоднозначный BOMB остался по тикеру и блокирует обе монеты с этим тикером.
    assert_eq!(p.ambiguous.len(), 1);
    for a in [b1, b2] {
        let coin = Asset {
            ticker: "BOMB".into(),
            bsc_contract: Some(a),
        };
        assert_eq!(
            blacklist::manual(&s, &coin, &[]),
            Some(Some("два токена".into()))
        );
    }
    // CAKE по адресу — блок действует.
    let c = Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(cake),
    };
    assert_eq!(blacklist::manual(&s, &c, &[]), Some(Some("скам".into())));
    // Повтор: переносить нечего, значения не меняются.
    let p2 = plan(&old_ids(&s), &by);
    assert!(p2.moves.is_empty());
    assert_eq!(apply(&s, &p2).await.0, 0);
    assert_eq!(AUTHOR, "миграция решения 211");
}

#[tokio::test]
async fn address_value_not_overwritten() {
    let s = store("conflict").await;
    let a = Address::repeat_byte(7);
    let at = Scope::Coin(coin_id("X", Some(&a)));
    s.set(
        "coin.blocked_note",
        at.clone(),
        Some(json!("по адресу")),
        "тест",
    )
    .await
    .unwrap();
    s.set(
        "coin.blocked_note",
        Scope::Coin("X".into()),
        Some(json!("по тикеру")),
        "тест",
    )
    .await
    .unwrap();
    let mut by: BTreeMap<String, BTreeSet<Address>> = BTreeMap::new();
    by.entry("X".into()).or_default().insert(a);
    let (moved, conflicts, _) = apply(&s, &plan(&old_ids(&s), &by)).await;
    assert_eq!((moved, conflicts.len()), (0, 1));
    assert_eq!(
        s.get_exact("coin.blocked_note", &at),
        Some(json!("по адресу"))
    );
}

/// Значения убранных решением 211 настроек (coin.ticker_alias, область «монета» неизвестной настройки) в базе: хранилище
/// загружается, значение видно и очищается (решение 59), перенос его не трогает.
#[tokio::test]
async fn removed_settings_in_db_do_not_break() {
    let d = std::env::temp_dir().join(format!("arb-coin-scope-removed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let g = Scope::Global.to_key();
    let c = Scope::Coin("NMT".into()).to_key();
    db.call(move |conn| {
        for (n, sc, v) in [
            ("coin.ticker_alias", g.as_str(), "[\"0x03aa=NMT,NETMIND\"]"),
            ("coin.ticker_alias_auto", g.as_str(), "true"),
            ("coin.ticker_alias", c.as_str(), "\"x\""),
        ] {
            conn.execute(
                "INSERT INTO settings (name, scope, value, updated_at) VALUES (?1, ?2, ?3, '2026-11-07T00:00:00Z')",
                rusqlite::params![n, sc, v],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let s = Store::load(db.clone(), changes, Journal::new(db, jtx))
        .await
        .unwrap();
    assert_eq!(
        s.get_exact("coin.ticker_alias_auto", &Scope::Global),
        Some(json!(true))
    );
    assert!(
        old_ids(&s).is_empty(),
        "неизвестная настройка в области «монета» не переносится"
    );
    s.set("coin.ticker_alias_auto", Scope::Global, None, "тест")
        .await
        .unwrap();
    assert_eq!(s.get_exact("coin.ticker_alias_auto", &Scope::Global), None);
}
