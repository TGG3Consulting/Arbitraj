//! Файл: tests/bingx_poll_263.rs
//! Что делает: решение 263 (А3) — доведение ордера BingX (venues/bingx/order_state.rs settle) на настоящем транспорте
//! адаптера: запросы идут по HTTP в локальный сервер. Неудачный опрос ордера не даёт сразу «итог неизвестен»: опрос
//! повторяется до venue.order_final_wait_ms, затем отмена остатка и снова опрос до того же срока.
//! Ответы сервера: сбой — {"code":100500,"msg":"System busy"} (код и текст — таблица кодов BingX, «100500 System
//! busy»); состояние ордера — пример ответа из официальной документации docs-v3 (Spot → Trade → Create an Order,
//! «Response», status PENDING — тот же, что в tests/bingx_rate_pause.rs). Окончательного ответа Query Order BingX в
//! документации-примерах и в tests/data нет (tests/README.md) — проверяется повтор и итог по сроку, без выдуманных данных.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bingx (ctx, order_state), arb::settings, arb::storage, arb::secrets, arb::gateway.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use arb::gateway::errors::ErrorCode;
use arb::secrets::crypto::KdfParams;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bingx::{ctx::Ctx, order_state};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use zeroize::Zeroizing;

const QUERY: &str = "/openApi/spot/v1/trade/query";
const CANCEL: &str = "/openApi/spot/v1/trade/cancel";
/// Пример ответа Create an Order (docs-v3, «Response»): status PENDING — не окончательный.
const ORDER_DOC: &str = r#"{"code":0,"msg":"","debugMsg":"","data":{"symbol":"NEMS-USDT","orderId":1735963671248581120,"transactTime":1702720966528,"price":"0.1557","origQty":"115","executedQty":"0","cummulativeQuoteQty":"0","status":"PENDING","type":"LIMIT","side":"SELL"}}"#;
/// Ответ BingX со сбоем «System busy» (код 100500 таблицы кодов).
const BUSY: &str = r#"{"code":100500,"msg":"System busy"}"#;
const KDF: KdfParams = KdfParams {
    memory_kib: 64,
    iterations: 1,
    parallelism: 1,
};

/// Ответы по путям: очередь тел; последнее повторяется. Счётчики запросов по путям.
type Plan = Arc<Mutex<HashMap<&'static str, Vec<&'static str>>>>;
type Hits = Arc<Mutex<Vec<String>>>;

/// Локальный сервер BingX. Возвращает: адрес, план, счётчики.
async fn serve() -> (String, Plan, Hits) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    let (plan, hits): (Plan, Hits) = Default::default();
    let (p, h) = (plan.clone(), hits.clone());
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let (p, h) = (p.clone(), h.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16 * 1024];
                let k = s.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..k]).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("");
                let path = path.split('?').next().unwrap_or("").to_string();
                h.lock().unwrap().push(path.clone());
                let body = {
                    let mut m = p.lock().unwrap();
                    match m.get_mut(path.as_str()) {
                        Some(q) if q.len() > 1 => q.remove(0),
                        Some(q) if !q.is_empty() => q[0],
                        _ => "{}",
                    }
                };
                let r = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(r.as_bytes()).await;
            });
        }
    });
    (addr, plan, hits)
}

/// Окружение адаптера BingX на SQLite во временной папке (ключи — в хранилище секретов), сроки ордера заданы.
async fn start() -> (Ctx, Plan, Hits) {
    let d = std::env::temp_dir().join(format!("arb-bingx-263-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = arb::settings::store::Store::load(db, changes, journal.clone())
        .await
        .unwrap();
    let (url, plan, hits) = serve().await;
    let g = |k: &str, v: Value| (k.to_string(), Scope::Global, v);
    let b = |k: &str, v: Value| (k.to_string(), Scope::Venue("bingx".into()), v);
    for (k, sc, v) in [
        g("http.timeout_ms", json!(5000)),
        g("http.body_idle_ms", json!(5000)),
        g("http.total_ms", json!(10000)),
        g("http.max_body_bytes", json!(1_000_000)),
        b("venue.rest_url", json!(url)),
        b("venue.recv_window_ms", json!(5000)),
        b("venue.order_poll_ms", json!(50)),
        b("venue.order_final_wait_ms", json!(300)),
    ] {
        settings.set(&k, sc, Some(v), "тест").await.unwrap();
    }
    let vault = arb::secrets::Vault::locked(d.join("secrets.enc"));
    let pw = || Zeroizing::new("pw".to_string());
    vault.create(pw(), KDF).await.unwrap();
    for (n, v) in [("bingx.api_key", "k"), ("bingx.api_secret", "s")] {
        let v = Some(Zeroizing::new(v.to_string()));
        vault.change(pw(), n, v, KDF).await.unwrap();
    }
    let ctx = Ctx {
        settings: settings.clone(),
        vault,
        http: reqwest::Client::new(),
        journal,
        raw: RawLog::start(d.join("raw"), settings),
        taps: Default::default(),
    };
    (ctx, plan, hits)
}

#[tokio::test]
async fn failed_poll_is_retried_until_deadline() {
    let (ctx, plan, hits) = start().await;
    // Первый опрос — сбой, дальше ордер в PENDING (не окончательный); отмена — тоже сбой.
    plan.lock().unwrap().insert(QUERY, vec![BUSY, ORDER_DOC]);
    plan.lock().unwrap().insert(CANCEL, vec![BUSY]);
    let e = order_state::settle(&ctx, "NEMS-USDT", "1735963671248581120")
        .await
        .unwrap_err();
    let of = |p: &str| hits.lock().unwrap().iter().filter(|x| *x == p).count();
    // Было: первый же сбой опроса — сразу «итог неизвестен», 1 запрос, без отмены.
    assert!(of(QUERY) >= 4, "опрос повторялся: {} запросов", of(QUERY));
    assert_eq!(of(CANCEL), 1, "после срока — одна отмена остатка");
    assert_eq!(e.code, ErrorCode::OutcomeUnknown, "{}", e.message);
    assert!(e.message.contains("статус PENDING"), "{}", e.message);
    assert!(e.message.contains("отмена: "), "{}", e.message);
}
