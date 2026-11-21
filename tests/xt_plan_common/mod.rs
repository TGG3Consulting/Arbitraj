//! Файл: tests/xt_plan_common/mod.rs
//! Что делает: общее для tests/core_plan_limit_262.rs (решение 262) — локальный HTTP-сервер XT с записанными ответами
//! (справочник пар GET /v4/public/symbol — живой ответ 06.11, tests/data/xt_live_2026-11-06/symbol_pepe.json; стакан
//! GET /v4/public/depth — пример официальной документации XT, tests/data/rest_book_2026-11-09/xt_depth_doc.json) со
//! счётчиком запросов стакана и живое ядро на SQLite во временной папке с настоящим адаптером XT.
//! Кто вызывает: tests/core_plan_limit_262.rs.
//! От чего зависит: arb::core, arb::venues::xt, arb::settings, arb::storage, arb::gateway.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use arb::core::Core;
use arb::core::deps::Deps;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

/// Локальный сервер XT с записанными ответами (справочник пар, стакан REST). Возвращает: адрес, счётчик стакана.
pub async fn serve() -> (String, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    let depth_n = Arc::new(AtomicUsize::new(0));
    let depth =
        std::fs::read_to_string("tests/data/rest_book_2026-11-09/xt_depth_doc.json").unwrap();
    let t = std::fs::read_to_string("tests/data/xt_live_2026-11-06/symbol_pepe.json").unwrap();
    let result: Value = serde_json::from_str(&t).unwrap();
    let symbols = json!({"rc": 0, "mc": "SUCCESS", "ma": [], "result": result}).to_string();
    let n = depth_n.clone();
    tokio::spawn(async move {
        while let Ok((mut c, _)) = l.accept().await {
            let (depth, symbols, n) = (depth.clone(), symbols.clone(), n.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16 * 1024];
                let k = c.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..k]).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                let (code, body) = if path.starts_with("/v4/public/depth") {
                    n.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", depth)
                } else if path.starts_with("/v4/public/symbol") {
                    ("200 OK", symbols)
                } else {
                    ("404 Not Found", "{}".to_string())
                };
                let r = format!(
                    "HTTP/1.1 {code}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = c.write_all(r.as_bytes()).await;
            });
        }
    });
    (addr, depth_n)
}

/// Ядро с адаптером XT на базе в новой папке. Возвращает: ядро, счётчик запросов стакана.
pub async fn start(name: &str) -> (Core, Arc<AtomicUsize>) {
    let d = std::env::temp_dir().join(format!("arb-262-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let (url, n) = serve().await;
    let vault = arb::secrets::Vault::locked(Path::new(&d).join("secrets.enc"));
    let g = |k: &str, v: Value| (k.to_string(), Scope::Global, v);
    let x = |k: &str, v: Value| (k.to_string(), Scope::Venue("xt".into()), v);
    for (k, sc, v) in [
        g("http.timeout_ms", json!(5000)),
        g("http.body_idle_ms", json!(5000)),
        g("http.total_ms", json!(10000)),
        g("http.max_body_bytes", json!(10_000_000)),
        x("venue.rest_url", json!(url)),
        x("venue.quote_asset", json!("USDT")),
        x("venue.symbols_refresh_s", json!(3600)),
        x("feed.stale_after_ms", json!(30000)),
        x("venue.book_stale_ms", json!(5000)),
        x("venue.book_stream", json!("depth")),
        x("venue.depth_levels", json!(20)),
        x("venue.ws_url", json!("wss://stream.xt.com/public")),
        x("venue.ws_ping_ms", json!(20000)),
        x("venue.ws_silence_ms", json!(30000)),
        x("venue.ws_subs_per_sec", json!(10)),
        x("venue.ws_streams_per_msg", json!(10)),
        x("venue.ws_reconnect_pause_ms", json!(1000)),
    ] {
        settings.set(&k, sc, Some(v), "тест").await.unwrap();
    }
    let ctx = arb::venues::xt::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: reqwest::Client::new(),
        journal: journal.clone(),
        raw: RawLog::start(d.join("raw"), settings.clone()),
    };
    let gw = Registry::new();
    gw.register(Arc::new(arb::venues::xt::Xt::new(ctx))).await;
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault,
        journal: journal.clone(),
    };
    let data_dir = d.to_path_buf();
    let (buffer, gw) = (64, gw);
    let deps = Deps {
        gw,
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer,
        data_dir,
    };
    (Core::start(deps).await, n)
}
