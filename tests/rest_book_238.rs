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
    db: Db,
    changes: broadcast::Sender<arb::settings::live::Change>,
    dir: std::path::PathBuf,
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
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let raw = RawLog::start(d.join("raw"), settings.clone());
    let vault = arb::secrets::Vault::locked(Path::new(&d).join("secrets.enc"));
    Env {
        db,
        changes,
        dir: d,
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
        chain: None,
        ticker: t.into(),
        contract: None,
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

/// Один и тот же запрос с пустым кэшем ставки: отказ REST до стакана отличается от
/// отказа после разбора полученных уровней. На повторе видны та же стадия и пара.
#[tokio::test]
async fn gate_rest_quote_identifies_the_stage_of_failure() {
    use arb::gateway::contract::Venue;
    use axum::{Json, Router, routing::get};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let e = env("gate-quote-stages").await;
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    let levels = read("tests/data/cex_samples_2026_11_04/gate_spot_order_book_CAKE_USDT.json");
    let markets = read("tests/data/cex_samples_2026_11_04/gate_spot_currency_pairs.json");
    let ace_market = markets
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == "ACE_USDT")
        .unwrap()
        .clone();
    let fee_reply = Arc::new(tokio::sync::Notify::new());
    let release = fee_reply.clone();
    let fee_started = Arc::new(tokio::sync::Notify::new());
    let started = fee_started.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route(
            "/spot/currency_pairs",
            get(move || async move { Json(json!([ace_market])) }),
        )
        .route(
            "/account/detail",
            get(|| async { Json(json!({"currency_pairs":[]})) }),
        )
        .route(
            "/wallet/fee",
            get(move || async move {
                started.notify_one();
                release.notified().await;
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    Json(json!({}))
                } else {
                    Json(json!({"maker_fee":"0.001","taker_fee":"0.001"}))
                }
            }),
        )
        .route(
            "/spot/order_book",
            get(move || {
                let body = if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    json!({})
                } else {
                    levels.clone()
                };
                async move { Json(body) }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for (key, val) in [
        ("http.timeout_ms", 5000),
        ("http.total_ms", 5000),
        ("http.body_idle_ms", 5000),
        ("http.max_body_bytes", 100000),
    ] {
        e.settings
            .set(key, Scope::Global, Some(json!(val)), "тест")
            .await
            .unwrap();
    }
    for (key, val) in [
        ("venue.rest_url", json!(url)),
        ("venue.quote_asset", json!("USDT")),
        ("venue.depth_levels", json!(20)),
        ("venue.ws_symbols_per_conn", json!(5)),
        ("venue.book_interval", json!("100ms")),
        ("venue.symbols_refresh_s", json!(3600)),
        ("venue.fee_requests_per_sec", json!(5)),
        ("venue.fee_cache_s", json!(3600)),
        ("venue.fee_retry_s", json!(1)),
        ("venue.fee_deduction", json!("coin")),
        ("venue.book_stale_ms", json!(5000)),
        ("feed.stale_after_ms", json!(30000)),
        ("venue.rate_limit_pause_ms", json!(30000)),
    ] {
        e.settings
            .set(key, Scope::Venue("gate".into()), Some(val), "тест")
            .await
            .unwrap();
    }
    let gate = Arc::new(arb::venues::gate::Gate::new(arb::venues::gate::ctx::Ctx {
        settings: e.settings.clone(),
        vault: e.vault.clone(),
        journal: e.journal.clone(),
        raw: e.raw,
        http: reqwest::Client::new(),
        taps: Default::default(),
    }));
    let coin = asset("ACE");
    let first = gate.quote_rest(&coin, Side::Sell).await.err().unwrap();
    assert_eq!(first.code, ErrorCode::BadResponse);
    assert!(
        first.message.contains("не получен или не разобран"),
        "{}",
        first.message
    );
    for _ in 0..2 {
        let err = gate.quote_rest(&coin, Side::Sell).await.err().unwrap();
        assert_eq!(err.code, ErrorCode::StaleData);
        assert!(
            err.message.contains("REST-стакан ACE получен и разобран"),
            "{}",
            err.message
        );
        assert!(
            err.message.contains("ACE_USDT: ставка комиссии"),
            "{}",
            err.message
        );
    }
    assert_eq!(requests.load(Ordering::SeqCst), 3);

    // Отсутствующая ставка известна до публичного HTTP.
    // При появлении ставки внутри 30-секундного окна запрашивается свежий стакан.
    let gw = arb::gateway::registry::Registry::new();
    gw.register(gate.clone()).await;
    let notifier = arb::notify::dispatch::Notifier {
        http: reqwest::Client::new(),
        settings: e.settings.clone(),
        vault: e.vault.clone(),
        journal: e.journal.clone(),
    };
    let core = arb::core::Core::start(arb::core::deps::Deps {
        gw,
        settings: e.settings.clone(),
        journal: e.journal,
        notifier,
        db: e.db,
        changes: e.changes,
        buffer: 64,
        data_dir: e.dir,
    })
    .await;
    let venue = gate.id();
    let quote = || arb::core::exec_book::book(&core, &venue, &coin, Side::Sell);
    let reject = quote().await.err().unwrap();
    assert!(
        reject.message.contains("REST-стакан не запрошен")
            && reject.message.contains("вне текущего списка"),
        "{}",
        reject.message
    );
    assert!(
        quote()
            .await
            .err()
            .unwrap()
            .message
            .contains("не запрашивался")
    );
    assert_eq!(requests.load(Ordering::SeqCst), 3);
    let kdf = arb::secrets::crypto::KdfParams {
        memory_kib: 64,
        iterations: 1,
        parallelism: 1,
    };
    let pw = || zeroize::Zeroizing::new("gate fixture".to_owned());
    e.vault.create(pw(), kdf).await.unwrap();
    for name in ["gate.api_key", "gate.api_secret"] {
        e.vault
            .change(
                pw(),
                name,
                Some(zeroize::Zeroizing::new("k".to_owned())),
                kdf,
            )
            .await
            .unwrap();
    }
    let (out, _keep_rx) = tokio::sync::mpsc::channel(32);
    gate.subscribe_market(vec![coin.clone()], out)
        .await
        .unwrap();
    let wait = std::time::Duration::from_secs(5);
    tokio::time::timeout(wait, fee_started.notified())
        .await
        .unwrap();
    assert!(
        gate.rest_book_prerequisite(&coin)
            .unwrap_err()
            .message
            .contains("получение ставки началось")
    );
    fee_reply.notify_one();
    tokio::time::timeout(wait, async {
        loop {
            let err = gate.rest_book_prerequisite(&coin).unwrap_err();
            if err.message.contains("BadResponse") && err.message.contains("venue.fees ACE_USDT") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    let cooling = quote().await.err().unwrap().message;
    assert!(
        cooling.contains("не запрашивался") && cooling.contains("BadResponse"),
        "{cooling}"
    );
    assert_eq!(requests.load(Ordering::SeqCst), 3);
    tokio::time::timeout(wait, fee_started.notified())
        .await
        .unwrap();
    fee_reply.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while gate.rest_book_prerequisite(&coin).is_err() {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    let fresh = quote()
        .await
        .expect("готовая ставка разрешает REST сразу внутри окна");
    assert!(fresh.rest);
    assert_eq!(
        fresh.snap.quote(dec("1")).await.unwrap().costs[0].amount,
        dec("0.0018902")
    );
    assert_eq!(requests.load(Ordering::SeqCst), 4);
    assert!(quote().await.unwrap().rest);
    assert_eq!(requests.load(Ordering::SeqCst), 4);
    server.abort();
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
