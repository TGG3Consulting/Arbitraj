//! Файл: tests/quiet_rest_book_t79.rs
//! Что делает: t79 (Тигран 10.11: согласовано; дополнение к решениям 202/213) — тихая пара биржи по содержимому,
//! признанная «книга не менялась с переподписки» (core/feed_rest_ref.rs), торгуется по стакану из ТОГО ЖЕ ответа REST
//! ровно столько, сколько разрешает действующая мера свежести. Моков логики нет: логика сторожа ядра
//! (core/feed_confirm.rs apply) и память стаканов адаптера (book.rs, book_rest.rs) — настоящие; данные:
//! - KuCoin — ЖИВОЙ ответ Get Part OrderBook level2_20 и ЖИВЫЕ кадры /spotMarket/level2Depth50 (запись 04.11,
//!   tests/data/cex_samples_2026_11_04);
//! - BingX — ЖИВЫЕ кадры @depth (запись 23.10, tests/data/bingx) и ответ GET /openApi/spot/v1/market/depth — ПРИМЕР ИЗ
//!   ДОКУМЕНТАЦИИ docs-v3 (живого ответа REST в tests/data нет, как в tests/bingx_real_data.rs);
//! - MEXC — ответ GET /api/v3/depth — ПРИМЕР ИЗ ДОКУМЕНТАЦИИ (живых кадров и ответа REST MEXC в tests/data нет, как в
//!   tests/mexc_real_data.rs).
//!
//! Проверяется: «кадры потеряны» — стакана нет (Action::Lost без уровней); тот же ответ после переподписки —
//! Unchanged с уровнями ЭТОГО ответа и его временем; стакан ставится (время кадра пары — получение ответа) и торгуется
//! (freeze проходит проверку свежести, дальше — ставка комиссии, её в тесте нет) в пределах venue.book_stale_ms, после
//! — не торгуется (StaleData «кадра стакана пары нет»); следующая проверка того же ответа — Quiet (обычное
//! подтверждение решения 213); новый кадр заменяет снимок; кадр после отправки запроса — снимок не ставится; стакана
//! нет и пара не подписана в живой сессии — не ставится, привязана (bind) — ставится; MEXC: кадр с прошлым снимком
//! снимок REST не продлевает (touch = false).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{feed_confirm, feed_watch_logic, feed_rest_ref}, arb::venues::{kucoin, bingx, mexc},
//! arb::settings, arb::storage.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use arb::core::feed_confirm::{QuietCfg, Verdict, apply, verdict};
use arb::core::feed_rest_ref::RestBook;
use arb::core::feed_watch_logic::{Action, Cfg, State};
use arb::core::types::{Asset, OrderBook, Side, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::gateway::feed_health::{
    BookPrint, BookPrints, BookVersion, ConnHealth, FeedHealth, PairHealth,
};
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// Свежесть стакана пары в тесте (venue.book_stale_ms), мс.
const STALE_MS: i64 = 400;

/// Окружение адаптера: настройки, журнал, сырой журнал, закрытый файл секретов.
struct Env {
    settings: Store,
    journal: Journal,
    raw: RawLog,
    vault: arb::secrets::Vault,
}

async fn env(name: &str) -> Env {
    let d = std::env::temp_dir().join(format!("arb-t79-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let raw = RawLog::start(d.join("raw"), settings.clone());
    let vault = arb::secrets::Vault::locked(Path::new(&d).join("secrets.enc"));
    let at = Scope::Venue(name.into());
    for (n, v) in [
        ("feed.stale_after_ms", json!(30000)),
        ("venue.book_stale_ms", json!(STALE_MS)),
        ("venue.quote_asset", json!("USDT")),
    ] {
        settings.set(n, at.clone(), Some(v), "тест").await.unwrap();
    }
    Env {
        settings,
        journal,
        raw,
        vault,
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn asset(t: &str) -> Asset {
    Asset {
        ticker: t.into(),
        bsc_contract: None,
    }
}

/// Сторож ядра на тех же ответах REST, часы — настоящие (снимок ставится в память адаптера по ним): первая проверка —
/// «кадры потеряны» (стакана нет), переподписка, вторая проверка тем же ответом — Unchanged с уровнями ответа.
/// Принимает: уровни ответа REST, стакан в памяти, N. Возвращает: стакан из ответа (RestBook) второй проверки.
fn unchanged_book(rest: &BookPrint, ours: Option<BookPrint>, n: usize) -> RestBook {
    let c = Cfg {
        book_stale_ms: 5_000,
        dead_mult: 3,
        live_ms: 40_000,
        warmup_ms: 30_000,
        attempts: 3,
        interval_ms: 3_000,
        pause_ms: 60_000,
        dead_pct: 30,
        reconnect_min_ms: 120_000,
        reconnect_max_per_hour: 3,
        quiet: QuietCfg::Confirm {
            min_ms: 30_000,
            max_per_s: 5,
        },
    };
    let now = now_ms();
    let h = |since| FeedHealth {
        scheduled: false,
        per_pair: true,
        confirm: true,
        conns: vec![ConnHealth {
            conn: "c1".into(),
            alive_ms: Some(now),
            started_ms: Some(now - 200_000),
            pairs: vec![PairHealth {
                ticker: "CAKE".into(),
                last_ms: Some(now - 100_000),
                since_ms: since,
                last_id: None,
                last_changed_ms: None,
            }],
        }],
    };
    let ver = || {
        Ok(BookVersion {
            print: Some(BookPrints {
                levels: n,
                rest: rest.clone(),
                ours: ours.clone(),
            }),
            ..Default::default()
        })
    };
    let mut st = State::default();
    let a = apply(
        &h(now - 150_000),
        "c1",
        "CAKE",
        ver(),
        now - 60_000,
        now - 59_900,
        &c,
        &mut st,
    );
    // Кадры потеряны — переподписка; стакана из ответа нет (при Lost ничего не ставится).
    assert!(
        matches!(&a[..], [Action::Lost { .. }, Action::Resub { .. }]),
        "{a:?}"
    );
    // Переподписана после ответа; тот же ответ — книга не менялась с переподписки.
    let (sent, recv) = (now - 30, now - 20);
    let a = apply(
        &h(now - 50_000),
        "c1",
        "CAKE",
        ver(),
        sent,
        recv,
        &c,
        &mut st,
    );
    match &a[..] {
        [Action::Unchanged { book: Some(b), .. }] => {
            assert_eq!(b.levels, *rest);
            assert_eq!((b.sent_ms, b.recv_ms), (sent, recv));
            b.clone()
        }
        _ => panic!("{a:?}"),
    }
}

/// Следующая проверка: ответ тот же, стакан в памяти — поставленный снимок — обычное «тихая, подтверждена» (213).
fn next_check_quiet(rest: &BookPrint, ours: Option<BookPrint>, n: usize) {
    let p = PairHealth {
        ticker: "CAKE".into(),
        last_ms: Some(1),
        since_ms: 0,
        last_id: None,
        last_changed_ms: None,
    };
    let v = BookVersion {
        print: Some(BookPrints {
            levels: n,
            rest: rest.clone(),
            ours,
        }),
        ..Default::default()
    };
    assert_eq!(verdict(&p, &v, 2), Verdict::Quiet);
}

/// Ответ проверки свежести: Ok — стакан торгуется (дальше нет только ставки комиссии); Err — текст причины.
fn traded(r: Result<(), arb::gateway::errors::GatewayError>) -> Result<(), String> {
    match r {
        Ok(()) => Ok(()),
        Err(e) if e.message.contains("ставка комиссии") => Ok(()),
        Err(e) => {
            assert_eq!(e.code, ErrorCode::StaleData, "{}", e.message);
            Err(e.message)
        }
    }
}

/// KuCoin: живой ответ REST и живые кадры (запись 04.11).
#[tokio::test]
async fn kucoin_quiet_pair_trades_by_rest_book_within_freshness() {
    use arb::venues::kucoin::{book::Books, book_print, depth, fees::Fees};
    let e = env("kucoin").await;
    let ctx = arb::venues::kucoin::ctx::Ctx {
        settings: e.settings.clone(),
        vault: e.vault.clone(),
        http: reqwest::Client::new(),
        journal: e.journal.clone(),
        raw: e.raw.clone(),
        taps: Default::default(),
    };
    let dir = "tests/data/cex_samples_2026_11_04";
    let t = std::fs::read_to_string(format!("{dir}/kucoin_orderbook_level2_20_CAKE-USDT.json"))
        .unwrap();
    let rest_v: Value = serde_json::from_str::<Value>(&t).unwrap()["data"].clone();
    let n = 5;
    let rest = book_print::parse(&rest_v, n).unwrap();
    // Последний кадр Level50 записи — стакан, как его кладёт ws_conn.rs.
    let frames: Vec<(Vec<_>, Vec<_>)> = std::fs::read_to_string(format!("{dir}/kucoin_ws.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| v.get("msg").and_then(Value::as_str).map(str::to_string))
        .filter_map(|f| match depth::parse(&f) {
            depth::Msg::Depth { bids, asks, .. } => Some((bids, asks)),
            _ => None,
        })
        .collect();
    let (bids, asks) = frames.last().cloned().unwrap();
    let frame = || OrderBook {
        venue: VenueId("kucoin".into()),
        asset: asset("CAKE"),
        bids: bids.clone(),
        asks: asks.clone(),
        exchange_time: None,
        received_at: chrono::Utc::now(),
    };
    let books = Books::default();
    let fees = Fees::default();
    let alive = Arc::new(AtomicI64::new(now_ms()));
    let freeze = |b: &Books| {
        traded(
            b.freeze(&ctx, &fees, "CAKE-USDT", &asset("CAKE"), Side::Buy)
                .map(|_| ()),
        )
    };
    books.put(frame(), alive.clone());
    let old = books.print("CAKE", n);
    assert_ne!(
        old.as_ref(),
        Some(&rest),
        "книга записи менялась — иначе пара была бы Quiet"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    let rb = unchanged_book(&rest, old, n);
    // Ставится: время кадра пары — получение ответа; уровни — ответа.
    let b = books
        .put_rest(
            VenueId("kucoin".into()),
            "CAKE",
            &rb.levels,
            rb.sent_ms,
            rb.recv_ms,
        )
        .unwrap();
    assert_eq!(b.received_at.timestamp_millis(), rb.recv_ms);
    assert_eq!(books.print("CAKE", n).as_ref(), Some(&rest));
    alive.store(now_ms(), Ordering::Relaxed);
    freeze(&books).unwrap();
    next_check_quiet(&rest, books.print("CAKE", n), n);
    // Срок свежести (venue.book_stale_ms от получения ответа) истёк — не торгуется.
    tokio::time::sleep(Duration::from_millis(STALE_MS as u64 + 50)).await;
    alive.store(now_ms(), Ordering::Relaxed);
    let why = freeze(&books).unwrap_err();
    assert!(why.contains("кадра стакана пары нет"), "{why}");
    // Новый кадр заменяет снимок.
    books.put(frame(), alive.clone());
    assert_eq!(
        books.print("CAKE", n),
        BookPrint::top(&bids, &asks, n).into()
    );
    freeze(&books).unwrap();
    // Кадр пришёл после отправки запроса — снимок не ставится, стакан потока остаётся.
    let why = books
        .put_rest(
            VenueId("kucoin".into()),
            "CAKE",
            &rest,
            now_ms() - 1_000,
            now_ms(),
        )
        .unwrap_err();
    assert!(why.contains("стакан потока новее"), "{why}");
    assert_eq!(
        books.print("CAKE", n),
        BookPrint::top(&bids, &asks, n).into()
    );
    // Стакан снят и пара не подписана в живой сессии — не ставится; привязана к сессии — ставится.
    books.drop_ticker("CAKE");
    let why = books
        .put_rest(VenueId("kucoin".into()), "CAKE", &rest, now_ms(), now_ms())
        .unwrap_err();
    assert!(why.contains("не подписана"), "{why}");
    books.bind(&[(asset("CAKE"), "CAKE-USDT".into())], &alive);
    books
        .put_rest(VenueId("kucoin".into()), "CAKE", &rest, now_ms(), now_ms())
        .unwrap();
    freeze(&books).unwrap();
}

/// BingX: живые кадры (запись 23.10) и пример ответа REST из документации.
#[tokio::test]
async fn bingx_quiet_pair_trades_by_rest_book_within_freshness() {
    use arb::venues::bingx::{book::Books, book_print, depth, fees::Fees};
    let e = env("bingx").await;
    let ctx = arb::venues::bingx::ctx::Ctx {
        settings: e.settings.clone(),
        vault: e.vault.clone(),
        http: reqwest::Client::new(),
        journal: e.journal.clone(),
        raw: e.raw.clone(),
        taps: Default::default(),
    };
    let docs: Value = serde_json::from_str(
        r#"{"bids":[["42182.22","0.99114"],["42182.19","1.00643"],["42182.17","0.95406"],["42182.10","0.87420"],["42182.08","1.17445"]],"asks":[["42183.23","3.57139"],["42183.22","2.05573"],["42183.20","2.26065"],["42183.19","1.61695"],["42183.17","4.14752"]],"ts":1702720487674}"#,
    )
    .unwrap();
    let n = 5;
    let rest = book_print::parse(&docs, n).unwrap();
    let raw = std::fs::read_to_string("tests/data/bingx/ws_spot_2026-10-23.jsonl").unwrap();
    let (bids, asks, et) = raw
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["m"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .find_map(|m| match depth::parse(&m) {
            depth::Msg::Depth {
                bids,
                asks,
                exchange_time,
                ..
            } => Some((bids, asks, exchange_time)),
            _ => None,
        })
        .unwrap();
    let frame = || depth::to_book(asset("CAKE"), bids.clone(), asks.clone(), et);
    let books = Books::default();
    let fees = Fees::default();
    let alive = Arc::new(AtomicI64::new(now_ms()));
    let freeze = |b: &Books| {
        traded(
            b.freeze(&ctx, &fees, &asset("CAKE"), Side::Sell)
                .map(|_| ()),
        )
    };
    books.put(frame(), alive.clone());
    let old = books.print("CAKE", n);
    assert_ne!(old.as_ref(), Some(&rest));
    tokio::time::sleep(Duration::from_millis(50)).await;
    let rb = unchanged_book(&rest, old, n);
    let b = books
        .put_rest(
            VenueId("bingx".into()),
            "CAKE",
            &rb.levels,
            rb.sent_ms,
            rb.recv_ms,
        )
        .unwrap();
    assert_eq!(b.received_at.timestamp_millis(), rb.recv_ms);
    assert_eq!(books.print("CAKE", n).as_ref(), Some(&rest));
    alive.store(now_ms(), Ordering::Relaxed);
    freeze(&books).unwrap();
    next_check_quiet(&rest, books.print("CAKE", n), n);
    tokio::time::sleep(Duration::from_millis(STALE_MS as u64 + 50)).await;
    alive.store(now_ms(), Ordering::Relaxed);
    let why = freeze(&books).unwrap_err();
    assert!(why.contains("кадра стакана пары нет"), "{why}");
    books.put(frame(), alive.clone());
    assert_eq!(
        books.print("CAKE", n),
        BookPrint::top(&bids, &asks, n).into()
    );
    freeze(&books).unwrap();
    let why = books
        .put_rest(
            VenueId("bingx".into()),
            "CAKE",
            &rest,
            now_ms() - 1_000,
            now_ms(),
        )
        .unwrap_err();
    assert!(why.contains("стакан потока новее"), "{why}");
}

/// MEXC: пример ответа REST из документации; пара без кадров в сессии (привязана); кадр с прошлым снимком снимок REST
/// не продлевает.
#[tokio::test]
async fn mexc_rest_book_freshness_and_touch() {
    use arb::venues::mexc::{book::Books, book_print, fees::Fees};
    let e = env("mexc").await;
    let ctx = arb::venues::mexc::ctx::Ctx {
        settings: e.settings.clone(),
        vault: e.vault.clone(),
        http: reqwest::Client::new(),
        journal: e.journal.clone(),
        raw: e.raw.clone(),
        taps: Default::default(),
    };
    let docs: Value = serde_json::from_str(
        r#"{"lastUpdateId":1112416,"bids":[["15.00000","49999.00000"]],"asks":[["14.0000","1.0000"]]}"#,
    )
    .unwrap();
    let n = 5;
    let rest = book_print::parse(&docs, n).unwrap();
    let books = Books::default();
    let fees = Fees::default();
    let alive = Arc::new(AtomicI64::new(now_ms()));
    let freeze = |b: &Books| {
        traded(
            b.freeze(&ctx, &fees, &asset("XTER"), Side::Sell)
                .map(|_| ()),
        )
    };
    // Стакана нет и пара не подписана — не ставится; подписана в сессии — ставится.
    assert!(
        books
            .put_rest(VenueId("mexc".into()), "XTER", &rest, now_ms(), now_ms())
            .is_err()
    );
    books.bind(&[(asset("XTER"), "XTERUSDT".into())], &alive);
    let recv = now_ms();
    books
        .put_rest(VenueId("mexc".into()), "XTER", &rest, recv - 10, recv)
        .unwrap();
    freeze(&books).unwrap();
    // Кадр с тем же снимком, что прошлый кадр потока (решение 109), снимок REST не продлевает — кадр кладётся заново.
    assert!(!books.touch("XTER"));
    tokio::time::sleep(Duration::from_millis(STALE_MS as u64 + 50)).await;
    alive.store(now_ms(), Ordering::Relaxed);
    let why = freeze(&books).unwrap_err();
    assert!(why.contains("кадра стакана пары нет"), "{why}");
    let frame =
        arb::gateway::rest_book::from_print(VenueId("mexc".into()), asset("XTER"), &rest, now_ms())
            .unwrap();
    books.put(frame, alive.clone());
    assert!(books.touch("XTER"));
    freeze(&books).unwrap();
}
