//! Файл: tests/bingx_293_common/mod.rs
//! Что делает: общее для тестов решения 293 — окружение адаптера BingX на SQLite во временной папке (без сети и
//! ключей), записанные ответы tests/data/bingx_minqty_2026-11-15, монета SHOPINX с адресом контракта BSC из
//! tests/data/shopinx_trades_2026-11-15/lot1.txt, справочник пар тем же путём, что Symbols::refresh (symbols_qty::take),
//! запись venue.ignore_deprecated_qty у монеты тем же ключом, что интерфейс (settings/scope.rs coin_id — решение 211).
//! Кто вызывает: tests/bingx_minqty_293.rs, tests/bingx_minqty_coin_293.rs.
//! От чего зависит: arb::venues::bingx, arb::settings, arb::storage, arb::secrets.
#![allow(dead_code)]

use std::str::FromStr;

use alloy_primitives::Address;
use arb::core::types::Asset;
use arb::settings::scope::coin_id;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bingx::ctx::Ctx;
use arb::venues::bingx::symbols::Symbols;
use arb::venues::bingx::symbols_qty;
use chrono::{TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

pub const DIR: &str = "tests/data/bingx_minqty_2026-11-15";
/// Пары выборки (файлы symbols_<ПАРА>.json и book_ticker_<ПАРА>.json).
pub const PAIRS: [&str; 9] = [
    "SHOPINX-USDT",
    "CAKE-USDT",
    "TWT-USDT",
    "SFP-USDT",
    "BTC-USDT",
    "DOGE-USDT",
    "BNB-USDT",
    "MUBARAK-USDT",
    "BANANAS31-USDT",
];
/// Время запроса symbols (параметр timestamp записанного запроса, README папки).
pub const SYMBOLS_AT_MS: i64 = 1_794_742_830_000;

pub fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

pub fn read(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{DIR}/{name}")).unwrap()).unwrap()
}

/// Лучший bid записанного bookTicker пары.
pub fn bid(pair: &str) -> Decimal {
    let v = read(&format!("book_ticker_{pair}.json"));
    d(v["data"][0]["bidPrice"].as_str().unwrap())
}

/// Строки отчёта о чужих сделках SHOPINX (tests/data/shopinx_trades_2026-11-15/lot1.txt).
pub fn lot1() -> String {
    std::fs::read_to_string("tests/data/shopinx_trades_2026-11-15/lot1.txt").unwrap()
}

/// Адрес контракта BSC SHOPINX — из шапки отчёта («Контракт BSC 0x…»).
pub fn shopinx_contract() -> Address {
    let t = lot1();
    let i = t.find("Контракт BSC ").unwrap() + "Контракт BSC ".len();
    Address::from_str(&t[i..i + 42]).unwrap()
}

/// Монета SHOPINX с адресом контракта (как в запросах ядра).
pub fn shopinx() -> Asset {
    Asset {
        chain: None,
        ticker: "SHOPINX".into(),
        contract: Some(shopinx_contract()),
    }
}

/// Монета без адреса (значение монеты к ней не применяется).
pub fn bare(ticker: &str) -> Asset {
    Asset {
        chain: None,
        ticker: ticker.into(),
        contract: None,
    }
}

/// Окружение адаптера BingX на SQLite во временной папке. Принимает: имя папки.
pub async fn ctx(tag: &str) -> Ctx {
    ctx_raw(tag, &[]).await
}

/// Как ctx, но до загрузки хранилища в таблицу settings пишутся строки как есть (имя, ключ области, значение JSON) —
/// так в базе оказывается значение неверного вида (повреждение), которое Store::set не пропустит.
pub async fn ctx_raw(tag: &str, rows: &[(&str, &str, &str)]) -> Ctx {
    let dir = std::env::temp_dir().join(format!("arb-bingx-293-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let rows: Vec<(String, String, String)> = rows
        .iter()
        .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
        .collect();
    db.call(move |c| {
        for (n, sc, v) in &rows {
            c.execute(
                "INSERT INTO settings (name, scope, value, updated_at) VALUES (?1, ?2, ?3, '2026-11-15T00:00:00Z')",
                rusqlite::params![n, sc, v],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    // Настройки окружения теста: котируемая монета и срок ордера (IOC — у BingX по документации place order).
    set(&settings, "venue.quote_asset", Some(json!("USDT"))).await;
    set(&settings, "venue.order_time_in_force", Some(json!("IOC"))).await;
    Ctx {
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(dir.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
        raw: RawLog::start(dir.join("raw"), settings),
        taps: Default::default(),
    }
}

/// Задать или снять настройку области «bingx».
pub async fn set(s: &Store, name: &str, v: Option<Value>) {
    s.set(name, Scope::Venue("bingx".into()), v, "тест")
        .await
        .unwrap();
}

/// venue.ignore_deprecated_qty у монеты — ключом интерфейса (coin_id: адрес строчными, решение 211).
pub async fn set_coin(s: &Store, a: &Asset, v: bool) {
    let sc = Scope::Coin(coin_id(&a.ticker, a.contract.as_ref()));
    s.set(symbols_qty::SETTING, sc, Some(json!(v)), "тест")
        .await
        .unwrap();
}

/// Справочник из записанного ответа SHOPINX тем же путём, что Symbols::refresh.
pub fn load(c: &Ctx) -> Symbols {
    let s = Symbols::default();
    let v = read("symbols_SHOPINX-USDT.json");
    let at = Utc.timestamp_millis_opt(SYMBOLS_AT_MS).unwrap();
    assert_eq!(symbols_qty::take(c, &s, &v["data"], at).unwrap(), 1);
    s
}

/// Справочник всех 9 пар: массивы symbols записанных ответов подряд (одним ответом), тем же путём, что refresh.
pub fn load_all(c: &Ctx) -> Symbols {
    let mut all = vec![];
    for pair in PAIRS {
        let v = read(&format!("symbols_{pair}.json"));
        all.extend(v["data"]["symbols"].as_array().unwrap().iter().cloned());
    }
    let s = Symbols::default();
    let at = Utc.timestamp_millis_opt(SYMBOLS_AT_MS).unwrap();
    let data = json!({ "symbols": all });
    assert_eq!(symbols_qty::take(c, &s, &data, at).unwrap(), PAIRS.len());
    s
}

/// Есть ли у пары монеты пределы количества — и в правилах ордера (get_for), и в состоянии для ядра (state_for).
pub fn has_qty(s: &Symbols, c: &Ctx, a: &Asset) -> bool {
    let r = s.get_for(c, a).unwrap();
    let p = s.state_for(a).unwrap();
    assert_eq!((r.min_qty, r.max_qty), (p.min_qty, p.max_qty));
    r.min_qty.is_some() && r.max_qty.is_some()
}
