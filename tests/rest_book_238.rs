//! Файл: tests/rest_book_238.rs
//! Что делает: решение 238 на уровне адаптеров (моков нет):
//! - ВХОД: стакан MEXC, кадр пары которого старше venue.book_stale_ms (как XTER 08.11: 32 с при 5000 мс), — котировки
//!   нет (book.rs freeze: StaleData «кадра стакана пары нет»), действующее подтверждение тихой пары (решение 213) —
//!   есть; BingX (раньше настройка необязательна) — venue.book_stale_ms не задана: котировки нет (NotConfigured), а не
//!   «свежесть только по соединению»;
//! - разбор снимков стакана REST (контракт quote_rest; у Phemex и Poloniex — book_print.rs levels_of, тот же разбор,
//!   что у подтверждения тихой пары) на записанных ответах: Gate — живой ответ GET /spot/order_book
//!   04.11 (tests/data/cex_samples_2026_11_04/gate_spot_order_book_CAKE_USDT.json); XT, Phemex, Poloniex — примеры
//!   ответов официальной документации (tests/data/rest_book_2026-11-09, README там же).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::{mexc, bingx, gate, xt, phemex, poloniex}, arb::settings, arb::storage.

use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use arb::core::types::{Asset, OrderBook, PriceLevel, Side, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// Окружение адаптера: настройки, журнал, сырой журнал, закрытый файл секретов.
struct Env {
    settings: Store,
    journal: Journal,
    raw: RawLog,
    vault: arb::secrets::Vault,
}

async fn env(name: &str) -> Env {
    let d = std::env::temp_dir().join(format!("arb-238a-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let raw = RawLog::start(d.join("raw"), settings.clone());
    let vault = arb::secrets::Vault::locked(Path::new(&d).join("secrets.enc"));
    Env {
        settings,
        journal,
        raw,
        vault,
    }
}

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn lv(p: &str, q: &str) -> PriceLevel {
    PriceLevel {
        price: dec(p),
        qty: dec(q),
    }
}

fn asset(t: &str) -> Asset {
    Asset {
        ticker: t.into(),
        bsc_contract: None,
    }
}

/// Стакан площадки с одним уровнем на сторону.
fn book(v: &str, t: &str) -> OrderBook {
    OrderBook {
        venue: VenueId(v.into()),
        asset: asset(t),
        bids: vec![lv("0.0073", "10000")],
        asks: vec![lv("0.0074", "10000")],
        exchange_time: None,
        received_at: chrono::Utc::now(),
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[tokio::test]
async fn mexc_stale_pair_has_no_quote_unless_confirmed() {
    let e = env("mexc").await;
    let at = Scope::Venue("mexc".into());
    for (n, v) in [
        ("feed.stale_after_ms", json!(30000)),
        ("venue.book_stale_ms", json!(50)),
        ("venue.quote_asset", json!("USDT")),
    ] {
        e.settings
            .set(n, at.clone(), Some(v), "тест")
            .await
            .unwrap();
    }
    let ctx = arb::venues::mexc::ctx::Ctx {
        settings: e.settings.clone(),
        vault: e.vault.clone(),
        http: reqwest::Client::new(),
        journal: e.journal.clone(),
        raw: e.raw.clone(),
        taps: Default::default(),
    };
    let books = arb::venues::mexc::book::Books::default();
    let fees = arb::venues::mexc::fees::Fees::default();
    let alive = Arc::new(AtomicI64::new(now_ms()));
    books.put(book("mexc", "XTER"), alive.clone());
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    // Соединение живо (кадры других пар идут), а кадра пары нет дольше venue.book_stale_ms.
    alive.store(now_ms(), std::sync::atomic::Ordering::Relaxed);
    let err = books
        .freeze(&ctx, &fees, &asset("XTER"), Side::Sell)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::StaleData);
    assert!(
        err.message.contains("кадра стакана пары нет"),
        "{}",
        err.message
    );
    assert!(
        err.message.contains("venue.book_stale_ms = 50"),
        "{}",
        err.message
    );
    // Подтверждение тихой пары до срока — проверка возраста пройдена (дальше — ставка комиссии, её здесь нет).
    books.confirm("XTER", now_ms() + 60_000);
    let err = books
        .freeze(&ctx, &fees, &asset("XTER"), Side::Sell)
        .unwrap_err();
    assert!(err.message.contains("ставка комиссии"), "{}", err.message);
}

#[tokio::test]
async fn bingx_requires_book_stale_ms() {
    let e = env("bingx").await;
    let at = Scope::Venue("bingx".into());
    e.settings
        .set("feed.stale_after_ms", at, Some(json!(30000)), "тест")
        .await
        .unwrap();
    let ctx = arb::venues::bingx::ctx::Ctx {
        settings: e.settings.clone(),
        vault: e.vault.clone(),
        http: reqwest::Client::new(),
        journal: e.journal.clone(),
        raw: e.raw.clone(),
        taps: Default::default(),
    };
    let books = arb::venues::bingx::book::Books::default();
    let fees = arb::venues::bingx::fees::Fees::default();
    books.put(book("bingx", "XTER"), Arc::new(AtomicI64::new(now_ms())));
    let err = books
        .freeze(&ctx, &fees, &asset("XTER"), Side::Sell)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotConfigured, "{}", err.message);
    assert!(
        err.message.contains("venue.book_stale_ms"),
        "{}",
        err.message
    );
}

fn read(p: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[tokio::test]
async fn gate_live_order_book_parses_to_book() {
    let e = env("gate").await;
    let ctx = arb::venues::gate::ctx::Ctx {
        settings: e.settings.clone(),
        vault: e.vault.clone(),
        http: reqwest::Client::new(),
        journal: e.journal.clone(),
        raw: e.raw.clone(),
        taps: Default::default(),
    };
    let v = read("tests/data/cex_samples_2026_11_04/gate_spot_order_book_CAKE_USDT.json");
    let b = arb::venues::gate::book_version::rest_book(&ctx, &v, &asset("CAKE")).unwrap();
    assert_eq!(b.asks[0], lv("1.896", "587.88"));
    assert_eq!(b.bids[0], lv("1.8902", "72.51"));
    assert_eq!(b.asks.len(), 20);
    assert!(b.asks.windows(2).all(|w| w[0].price < w[1].price));
    assert!(b.bids.windows(2).all(|w| w[0].price > w[1].price));
    assert_eq!(b.exchange_time.unwrap().timestamp_millis(), 1793740713013);
}

#[tokio::test]
async fn xt_doc_depth_parses_to_book() {
    let e = env("xt").await;
    let ctx = arb::venues::xt::ctx::Ctx {
        settings: e.settings.clone(),
        vault: e.vault.clone(),
        http: reqwest::Client::new(),
        journal: e.journal.clone(),
        raw: e.raw.clone(),
    };
    let v = read("tests/data/rest_book_2026-11-09/xt_depth_doc.json");
    let b = arb::venues::xt::book_version::rest_book(&ctx, &v["result"], &asset("BTC")).unwrap();
    assert_eq!(
        b.bids,
        vec![lv("200", "0.996"), lv("100", "0.001"), lv("20", "10")]
    );
    assert!(b.asks.is_empty());
}

#[test]
fn phemex_doc_orderbook_unscaled() {
    // Масштабы: цена — «All spot symbols use the same price … scale factors (price scale facor(Ep) = 8» (Public-Spot-
    // API-en.md); valueScale BTC = 8 — таблица «All Spot trading products» (tests/phemex_real_data.rs).
    let v = read("tests/data/rest_book_2026-11-09/phemex_orderbook_doc.json");
    let (bids, asks) = arb::venues::phemex::book_print::levels_of(&v["result"], 8, 8).unwrap();
    assert_eq!(asks[0], lv("8770.5", "0.01"));
    assert_eq!(bids[0], lv("8770", "0.02"));
    assert_eq!(bids[1], lv("8769.5", "0.002"));
}

#[test]
fn poloniex_doc_orderbook_flat_pairs() {
    let v = read("tests/data/rest_book_2026-11-09/poloniex_orderbook_doc.json");
    let (bids, asks) = arb::venues::poloniex::book_print::levels_of(&v).unwrap();
    assert_eq!(asks, vec![lv("1.91", "47.17277487")]);
    assert_eq!(bids, vec![lv("1.9", "110"), lv("1.889", "10")]);
    // Нечётная длина — не пары: ответ не разбирается по догадке.
    let bad = json!({"asks": ["1.91"], "bids": []});
    assert!(arb::venues::poloniex::book_print::levels_of(&bad).is_err());
}
