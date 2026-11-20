//! Файл: tests/size_common/dex.rs
//! Что делает: пул CAKE/USDT PancakeSwap V2 из записи узла (резервы getReserves на блоке 0x7b936ae) и окружение
//! адаптера DEX на настоящей SQLite (dex.v2_fee_pct = 0,25 — документация PancakeSwap V2, как настройка Тиграна).
//! Кто вызывает: tests/size_common/mod.rs.
//! От чего зависит: arb::venues::bsc_dex, arb::settings, arb::storage.

use alloy_primitives::U256;
use arb::core::types::{Pool, PoolRef, PoolState, VenueId};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::{db::Db, journal::Journal, raw_log::RawLog};
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::pools::{families, registry::Entry};
use chrono::Utc;
use serde_json::json;
use tokio::sync::broadcast;

use super::{CAKE, PAIR, USDT, json_file};

/// Пул CAKE/USDT V2 с записанными резервами.
pub fn pool() -> Entry {
    let node = json_file("tests/data/indexers_2026-10-24/arb_node_responses_3_2026-10-25.json");
    let res = node["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == format!("s1 CAKE pancakeswap_v2 {PAIR:#x} getReserves"))
        .unwrap();
    let hex = res["response"]["result"]
        .as_str()
        .unwrap()
        .trim_start_matches("0x")
        .to_string();
    let word = |i: usize| U256::from_str_radix(&hex[i * 64..(i + 1) * 64], 16).unwrap();
    let f = families::make("pancakeswap_v2", "pancakeswap_v2").unwrap();
    Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(PAIR),
            token0: CAKE,
            token1: USDT,
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::V2 {
                reserve0: word(0),
                reserve1: word(1),
            },
            block: 0x7b936ae,
            received_at: Utc::now(),
        },
        ticker: "CAKE".into(),
        coin: CAKE,
        usdt: USDT,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 0x7b936ae,
        synced_log: None,
        valid: true,
        asleep: false,
        state_gen: 0,
    }
}

/// Окружение адаптера DEX на настоящей SQLite: комиссия пары V2 — настройка dex.v2_fee_pct.
/// Каталог базы уникален на каждый вызов (pid + счётчик, как tests/route_common): тесты одного набора идут потоками
/// одного процесса, и два теста с одним `name` (Rec::load("uncapped-maxwin") в двух тестах tests/uncapped_maxwin.rs)
/// раньше делили один файл — remove_dir_all второго удалял базу, открытую первым (SQLite «disk I/O error», t138).
pub async fn ctx(name: &str) -> Ctx {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "arb-size-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let c = {
        let c = Ctx {
            raw: RawLog::start(dir.join("raw"), settings.clone()),
            pace: Default::default(),
            failover: Default::default(),
            place: arb::venues::bsc_dex::ctx::Place::BSC,
            chains: Default::default(),
            account: None,
            index: Default::default(),
            settings,
            vault: arb::secrets::Vault::locked(dir.join("secrets.enc")),
            http: reqwest::Client::new(),
            journal,
        };
        // Решение 416: тикер газа площадки DEX — настройка (dex.gas_ticker).
        c.settings
            .set(
                "dex.gas_ticker",
                arb::settings::scope::Scope::Venue("bsc_dex".into()),
                Some(serde_json::json!("BNB")),
                "тест",
            )
            .await
            .unwrap();
        c
    };
    let scope = Scope::Dex("pancakeswap_v2".into());
    c.settings
        .set("dex.v2_fee_pct", scope, Some(json!("0.25")), "тест")
        .await
        .unwrap();
    c
}
