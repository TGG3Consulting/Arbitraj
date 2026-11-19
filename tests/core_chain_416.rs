//! Файл: tests/core_chain_416.rs
//! Что делает: решение 416 (шаг 1 двух сетей) на записанных данных живого бота и настоящей SQLite:
//! (а) старая запись (направление сделки из выгрузки живого бота 16.11, без поля «chain») читается: сеть монеты не
//!     указана (None), а сеть по правилу core/chain.rs — сеть площадки DEX из её настройки dex.chain; запись сделки
//!     без сети пишется обратно байт в байт как раньше (ключа «chain» нет, адрес — «bsc_contract»);
//! (б) монета с тем же адресом в двух сетях — разные монеты (равенство, хэш, ключ CoinKey); в одной сети — одна;
//! (в) сеть монеты без сети: dex.chain не задана — ошибка; две площадки DEX с разными сетями — ошибка (шаг 2), с одной —
//!     её сеть.
//! Записи: tests/data/sigma_windows_2026-11-16/deals_sell_on_cex.jsonl (поле direction — как в учёте бота).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{chain, coin_key, deal, types}, arb::settings, arb::storage. Моков нет.

use std::collections::HashSet;

use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::chain;
use arb::core::coin_key::CoinKey;
use arb::core::deal::Deal;
use arb::core::types::{Asset, ChainId, Direction, VenueId};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;

const DEALS: &str = "tests/data/sigma_windows_2026-11-16/deals_sell_on_cex.jsonl";

/// Направления сделок из выгрузки живого бота (как записаны). Паника — файла нет.
fn recorded() -> Vec<Value> {
    std::fs::read_to_string(DEALS)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).unwrap()["direction"].clone())
        .collect()
}

/// Новая база настроек во временной папке.
async fn fresh(name: &str) -> Store {
    let dir = std::env::temp_dir().join(format!("arb-chain-416-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (tx, _) = broadcast::channel(16);
    let (jtx, _) = broadcast::channel(16);
    Store::load(db.clone(), tx, Journal::new(db, jtx))
        .await
        .unwrap()
}

/// Задаёт dex.chain площадке.
async fn set_chain(s: &Store, venue: &str, id: &str) {
    s.set(
        "dex.chain",
        Scope::Venue(venue.into()),
        Some(json!(id)),
        "тест",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn old_record_without_chain_reads_as_dex_chain() {
    let dirs = recorded();
    assert!(!dirs.is_empty(), "в выгрузке есть сделки");
    let s = fresh("old").await;
    let dex = [VenueId("bsc_dex".into())];
    set_chain(&s, "bsc_dex", "bsc").await;
    for raw in dirs {
        assert!(raw["asset"].get("chain").is_none(), "запись до решения 416");
        let d: Direction = serde_json::from_value(raw.clone()).unwrap();
        assert!(d.asset.contract.is_some(), "адрес из «bsc_contract»");
        assert!(d.asset.chain.is_none(), "сеть в записи не указана");
        assert_eq!(
            chain::of(&s, &dex, &d.asset).unwrap(),
            ChainId("bsc".into()),
            "монета без сети — сеть площадки DEX (dex.chain)"
        );
        // Запись без сети — байт в байт как раньше (и в составе сделки).
        assert_eq!(serde_json::to_value(&d).unwrap(), raw);
        let deal = Deal::new("t".into(), d.clone(), Value::Null, Decimal::ZERO);
        let text = serde_json::to_string(&deal).unwrap();
        let back: Deal = serde_json::from_str(&text).unwrap();
        assert_eq!(back.direction, d);
        assert_eq!(
            serde_json::to_value(&back).unwrap()["direction"],
            raw,
            "сделка пишет направление как раньше"
        );
    }
}

#[tokio::test]
async fn same_address_in_two_chains_are_two_coins() {
    let raw = recorded().remove(0);
    let old: Direction = serde_json::from_value(raw).unwrap();
    let addr = old.asset.contract;
    let on = |c: &str| Asset {
        ticker: old.asset.ticker.clone(),
        chain: Some(ChainId(c.into())),
        contract: addr,
    };
    let (bsc, arb) = (on("bsc"), on("arbitrum"));
    assert_ne!(bsc, arb, "один адрес в двух сетях — разные монеты");
    assert_ne!(CoinKey::of(&bsc), CoinKey::of(&arb));
    assert_eq!(bsc, on("bsc"), "та же сеть и адрес — одна монета");
    assert_eq!(CoinKey::of(&bsc), CoinKey::of(&on("bsc")));
    let set: HashSet<Asset> = [bsc.clone(), arb.clone(), on("bsc")].into_iter().collect();
    assert_eq!(set.len(), 2, "хэш согласован с равенством");
    // Без сети (шаг 1) — как раньше: равны по адресу, тикер не участвует.
    let mut renamed = old.asset.clone();
    renamed.ticker = "OTHER".into();
    assert_eq!(old.asset, renamed);
    assert_ne!(
        old.asset, bsc,
        "сеть не указана ≠ сеть указана (перенос — шаг 2)"
    );
    // Сеть пишется в JSON, только если указана; читается обратно.
    let v = serde_json::to_value(&arb).unwrap();
    assert_eq!(v["chain"], json!("arbitrum"));
    assert!(v.get("bsc_contract").is_some());
    let back: Asset = serde_json::from_value(v).unwrap();
    assert_eq!(back, arb);
    assert_eq!(back.chain, Some(ChainId("arbitrum".into())));
}

#[tokio::test]
async fn chain_of_coin_without_chain_needs_one_dex_chain() {
    let s = fresh("rule").await;
    let a: Direction = serde_json::from_value(recorded().remove(0)).unwrap();
    let one = [VenueId("bsc_dex".into())];
    let e = chain::of(&s, &one, &a.asset).unwrap_err();
    assert!(e.contains("dex.chain"), "{e}");
    assert!(chain::of(&s, &[], &a.asset).is_err(), "площадок DEX нет");
    set_chain(&s, "bsc_dex", "bsc").await;
    set_chain(&s, "arb_dex", "arbitrum").await;
    let two = [VenueId("bsc_dex".into()), VenueId("arb_dex".into())];
    let e = chain::dex_chain(&s, &two).unwrap_err();
    assert!(e.contains("разных сетях"), "{e}");
    // Своя сеть у монеты — она, без настроек площадок.
    let mut own = a.asset.clone();
    own.chain = Some(ChainId("arbitrum".into()));
    assert_eq!(
        chain::of(&s, &two, &own).unwrap(),
        ChainId("arbitrum".into())
    );
    assert_eq!(
        chain::of(&s, &one, &a.asset).unwrap(),
        ChainId("bsc".into())
    );
}
