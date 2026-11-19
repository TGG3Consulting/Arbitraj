//! Файл: tests/core_chain_430.rs
//! Что делает: решение 430 (шаг 2 двух сетей) — сети ядра на записанных данных живого бота и настоящей SQLite:
//! (а) одна площадка DEX — площадка DEX монеты без сети (запись живого бота) без чтения настроек, как раньше;
//! (б) две площадки DEX в разных сетях: сеть по умолчанию — dex.default_chain; не задана или не сеть площадки — ошибка;
//! (в) канонический вид: сеть по умолчанию — None (запись BSC прежняя), другая сеть — Some;
//! (г) площадка DEX сети: по сети монеты; у одной сети две площадки DEX — ошибка.
//! Записи: tests/data/sigma_windows_2026-11-16/deals_sell_on_cex.jsonl (поле direction — как в учёте бота).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{chain, types}, arb::settings, arb::storage. Моков нет.

use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::chain;
use arb::core::types::{ChainId, Direction, VenueId};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;

const DEALS: &str = "tests/data/sigma_windows_2026-11-16/deals_sell_on_cex.jsonl";

/// Первое направление сделки из выгрузки живого бота. Паника — файла нет.
fn recorded() -> Direction {
    let raw = std::fs::read_to_string(DEALS).unwrap();
    let line = raw.lines().find(|l| !l.trim().is_empty()).unwrap();
    serde_json::from_value(serde_json::from_str::<Value>(line).unwrap()["direction"].clone())
        .unwrap()
}

/// Новое хранилище настроек в своей папке. Паника — SQLite не открылась.
async fn fresh(name: &str) -> Store {
    let dir = std::env::temp_dir().join(format!("arb-chain-430-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (tx, _) = broadcast::channel(16);
    let (jtx, _) = broadcast::channel(16);
    Store::load(db.clone(), tx, Journal::new(db, jtx))
        .await
        .unwrap()
}

/// Задаёт настройку. Паника — запись не прошла.
async fn set(s: &Store, name: &str, scope: Scope, v: &str) {
    s.set(name, scope, Some(json!(v)), "тест").await.unwrap();
}

/// Площадка по id. Ошибок нет.
fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Сеть по id. Ошибок нет.
fn c(id: &str) -> ChainId {
    ChainId(id.into())
}

#[tokio::test]
async fn one_dex_answers_without_settings() {
    let s = fresh("one").await;
    let d = recorded();
    assert!(d.asset.chain.is_none(), "запись до решения 416");
    let one = [v("bsc_dex")];
    // dex.chain не задана — площадка DEX монеты без сети всё равно единственная (как «первая DEX» раньше).
    assert_eq!(
        chain::dex_of(&s, &one, d.asset.chain.as_ref()).unwrap(),
        v("bsc_dex")
    );
    set(&s, "dex.chain", Scope::Venue("bsc_dex".into()), "bsc").await;
    assert_eq!(
        chain::tag(&s, &one, &c("bsc")).unwrap(),
        None,
        "сеть по умолчанию — без сети"
    );
    assert_eq!(chain::dex_chain(&s, &one).unwrap(), c("bsc"));
}

#[tokio::test]
async fn two_chains_need_default_chain() {
    let s = fresh("two").await;
    let d = recorded();
    let two = [v("bsc_dex"), v("arb_dex")];
    set(&s, "dex.chain", Scope::Venue("bsc_dex".into()), "bsc").await;
    set(&s, "dex.chain", Scope::Venue("arb_dex".into()), "arbitrum").await;
    let e = chain::dex_of(&s, &two, d.asset.chain.as_ref()).unwrap_err();
    assert!(e.contains("dex.default_chain"), "{e}");
    set(&s, "dex.default_chain", Scope::Global, "solana").await;
    let e = chain::dex_chain(&s, &two).unwrap_err();
    assert!(e.contains("нет площадки DEX этой сети"), "{e}");
    set(&s, "dex.default_chain", Scope::Global, "bsc").await;
    assert_eq!(chain::dex_chain(&s, &two).unwrap(), c("bsc"));
    // Запись живого бота (без сети) — монета BSC, площадка bsc_dex; ключ записи не меняется.
    assert_eq!(chain::of(&s, &two, &d.asset).unwrap(), c("bsc"));
    assert_eq!(chain::dex_of(&s, &two, None).unwrap(), v("bsc_dex"));
    assert_eq!(chain::tag(&s, &two, &c("bsc")).unwrap(), None);
    assert_eq!(
        chain::tag(&s, &two, &c("arbitrum")).unwrap(),
        Some(c("arbitrum"))
    );
    assert_eq!(
        chain::dex_of(&s, &two, Some(&c("arbitrum"))).unwrap(),
        v("arb_dex")
    );
    let e = chain::dex_of(&s, &two, Some(&c("solana"))).unwrap_err();
    assert!(e.contains("нет площадки DEX сети solana"), "{e}");
}

#[tokio::test]
async fn one_chain_two_dex_is_error() {
    let s = fresh("same").await;
    let two = [v("bsc_dex"), v("bsc_dex2")];
    set(&s, "dex.chain", Scope::Venue("bsc_dex".into()), "bsc").await;
    set(&s, "dex.chain", Scope::Venue("bsc_dex2".into()), "bsc").await;
    let e = chain::dex_of(&s, &two, None).unwrap_err();
    assert!(e.contains("одна площадка DEX на сеть"), "{e}");
}
