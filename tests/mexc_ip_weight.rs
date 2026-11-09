//! Файл: tests/mexc_ip_weight.rs
//! Что делает: свой ограничитель веса публичных REST-запросов MEXC (Тигран 09.11: согласовано; venues/mexc/ip_weight.rs)
//! на настоящем адаптере: book_print::snapshot ходит по HTTP в локальный сервер, который отвечает примером ответа
//! GET /api/v3/depth из официальной документации (Market Data → Order Book,
//! https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/order-book; живой записи стакана REST MEXC в tests/data
//! нет — tests/README.md), и считает запросы. Моков логики нет. Проверяется:
//! - бюджет 3 (один depth, «Weight(IP): 3»): второй фоновый запрос ждёт, пока вес первого не выйдет из окна документации
//!   (10 с), а запрос пути сделки (котировка для исполнения, gateway/patience.rs exec_scope) идёт сразу и учитывается;
//! - бюджет меньше веса запроса — фоновый запрос отказ NotConfigured без запроса, путь сделки — идёт;
//! - настройка не задана — ограничителя нет (как было);
//! - вес пути без документации — отказ Internal без запроса.
//! Один тест на файл: состояние окна — общее на процесс (один адаптер MEXC, как пауза reply.rs).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::mexc (ctx, book_print, rest, ip_weight), arb::settings, arb::storage, arb::gateway.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use arb::core::types::Asset;
use arb::gateway::errors::ErrorCode;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::mexc::{book_print, ctx::Ctx, ip_weight, rest};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

/// Пример ответа из документации (Order Book, «Response»).
const DEPTH_DOC: &str =
    r#"{"lastUpdateId":1112416,"bids":[["15.00000","49999.00000"]],"asks":[["14.0000","1.0000"]]}"#;

/// Локальный сервер: /api/v3/depth — пример документации, остальное — 404. Возвращает: адрес и счётчик запросов.
async fn serve() -> (String, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    let n = Arc::new(AtomicUsize::new(0));
    let c = n.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            let c = c.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16 * 1024];
                let k = s.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..k]).to_string();
                c.fetch_add(1, Ordering::SeqCst);
                let (code, body) = if head.contains("/api/v3/depth") {
                    ("200 OK", DEPTH_DOC)
                } else {
                    ("404 Not Found", "{}")
                };
                let r = format!(
                    "HTTP/1.1 {code}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(r.as_bytes()).await;
            });
        }
    });
    (addr, n)
}

/// Окружение адаптера MEXC на SQLite во временной папке. Возвращает: окружение, настройки, счётчик запросов.
async fn start() -> (Ctx, Store, Arc<AtomicUsize>) {
    let d = std::env::temp_dir().join(format!("arb-mexc-ipw-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let (url, n) = serve().await;
    let g = |k: &str, v: Value| (k.to_string(), Scope::Global, v);
    let m = |k: &str, v: Value| (k.to_string(), Scope::Venue("mexc".into()), v);
    for (k, sc, v) in [
        g("http.timeout_ms", json!(5000)),
        g("http.body_idle_ms", json!(5000)),
        g("http.total_ms", json!(10000)),
        g("http.max_body_bytes", json!(1_000_000)),
        m("venue.rest_url", json!(url)),
        m("venue.quote_asset", json!("USDT")),
        m("venue.depth_levels", json!(5)),
    ] {
        settings.set(&k, sc, Some(v), "тест").await.unwrap();
    }
    let ctx = Ctx {
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
        raw: RawLog::start(d.join("raw"), settings.clone()),
        taps: Default::default(),
    };
    (ctx, settings, n)
}

fn coin() -> Asset {
    Asset {
        ticker: "MX".into(),
        bsc_contract: None,
    }
}

/// Задать или снять бюджет. Принимает: настройки, значение (None — снять).
async fn budget(s: &Store, v: Option<u64>) {
    s.set(
        ip_weight::SETTING,
        Scope::Venue("mexc".into()),
        v.map(|x| json!(x)),
        "тест",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn background_waits_trade_path_does_not() {
    let (ctx, s, n) = start().await;
    let q = || n.load(Ordering::SeqCst);
    // Вес пути без документации — отказ без запроса.
    assert_eq!(ip_weight::weight("/api/v3/ticker/price"), None);
    let e = rest::public(&ctx, "/api/v3/ticker/price", vec![])
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Internal, "{}", e.message);
    assert_eq!(q(), 0);
    // Не задана — ограничителя нет: три фоновых запроса сразу.
    let t = Instant::now();
    for _ in 0..3 {
        book_print::snapshot(&ctx, &coin()).await.unwrap();
    }
    assert_eq!(q(), 3);
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    // Бюджет меньше веса depth (3): фон — отказ NotConfigured без запроса, путь сделки — идёт.
    budget(&s, Some(2)).await;
    let e = book_print::snapshot(&ctx, &coin()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::NotConfigured, "{}", e.message);
    assert!(e.message.contains(ip_weight::SETTING), "{}", e.message);
    assert_eq!(q(), 3);
    arb::gateway::patience::exec_scope(book_print::snapshot(&ctx, &coin()))
        .await
        .unwrap();
    assert_eq!(q(), 4);
    // Окно очистилось (прошлые запросы шли без бюджета или срочно; ждём выхода их веса из окна документации).
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(ip_weight::used(), 0);
    // Бюджет 3 — один depth за 10 с: второй фоновый ждёт, срочный идёт сразу.
    budget(&s, Some(3)).await;
    let t = Instant::now();
    book_print::snapshot(&ctx, &coin()).await.unwrap();
    assert_eq!(q(), 5);
    let c2 = ctx.clone();
    let bg = tokio::spawn(async move {
        book_print::snapshot(&c2, &coin()).await.unwrap();
        Instant::now()
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(q(), 5, "фоновый ждёт места в окне");
    let u = Instant::now();
    arb::gateway::patience::exec_scope(book_print::snapshot(&ctx, &coin()))
        .await
        .unwrap();
    assert!(
        u.elapsed() < Duration::from_secs(2),
        "путь сделки не ждёт: {:?}",
        u.elapsed()
    );
    assert_eq!(q(), 6);
    assert_eq!(ip_weight::used(), 6, "вес срочного учтён");
    let done = bg.await.unwrap();
    assert!(
        done.duration_since(t) >= Duration::from_secs(10),
        "фоновый ушёл не раньше выхода веса из окна: {:?}",
        done.duration_since(t)
    );
    assert_eq!(q(), 7);
}
