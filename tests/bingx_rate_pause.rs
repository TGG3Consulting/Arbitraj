//! Файл: tests/bingx_rate_pause.rs
//! Что делает: решение 244 — пауза REST BingX после HTTP 429 (только эндпоинт) и 418 (вся биржа) на настоящем
//! транспорте адаптера (venues/bingx/rest.rs → rate_pause.rs): запросы идут по HTTP в локальный сервер, который отвечает
//! 429 / 418 или примером ответа из официальной документации docs-v3 (Spot → Market Data → Order Book и Spot → Trade →
//! Create an Order, «Response»; записанных ответов 429/418 и ордера BingX в tests/data нет — tests/README.md). Вид тела
//! 429/418 в документации BingX не описан — сервер отдаёт обёртку {code, msg} (пауза ставится по HTTP-статусу, тело не
//! важно). Моков логики нет. Проверяется:
//! - venue.rate_limit_pause_ms не задана — паузы нет (как было), запись Warn в журнал;
//! - 429 стакана — пауза только этого эндпоинта: стакан — RateLimited без запроса, ордер (подписанный POST) проходит;
//! - Retry-After в ответе — длительность по нему;
//! - 418 — пауза всей биржи (ордер — RateLimited без запроса), журнал Error, событие venue.rate_limited ядру;
//! - пауза кончилась — запросы снова идут.
//!
//! Один тест на файл: паузы — общие на процесс (один адаптер BingX, как reply.rs Bitget).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bingx (ctx, rest, rate_pause), arb::settings, arb::storage, arb::secrets, arb::gateway.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use arb::gateway::errors::ErrorCode;
use arb::gateway::events::MarketEvent;
use arb::secrets::crypto::KdfParams;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::{Journal, Level};
use arb::storage::raw_log::RawLog;
use arb::venues::bingx::{ctx::Ctx, rate_pause, rest};
use reqwest::Method;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};
use zeroize::Zeroizing;

const DEPTH: &str = "/openApi/spot/v1/market/depth";
const SYMBOLS: &str = "/openApi/spot/v1/common/symbols";
const ORDER: &str = "/openApi/spot/v1/trade/order";
/// Пример ответа Order Book (docs-v3, «Response»), сокращён до одного уровня.
const DEPTH_DOC: &str = r#"{"code":0,"timestamp":1702720487674,"data":{"bids":[["42182.22","0.99114"]],"asks":[["42183.23","3.57139"]],"ts":1702720487674}}"#;
/// Пример ответа Create an Order (docs-v3, «Response»).
const ORDER_DOC: &str = r#"{"code":0,"msg":"","debugMsg":"","data":{"symbol":"NEMS-USDT","orderId":1735963671248581120,"transactTime":1702720966528,"price":"0.1557","origQty":"115","executedQty":"0","cummulativeQuoteQty":"0","status":"PENDING","type":"LIMIT","side":"SELL"}}"#;
const KDF: KdfParams = KdfParams {
    memory_kib: 64,
    iterations: 1,
    parallelism: 1,
};

/// Что отвечает сервер: путь → (статус, Retry-After). Пути нет в карте — 200 с примером документации.
type Plan = Arc<Mutex<Vec<(&'static str, u16, Option<u64>)>>>;

/// Счётчики запросов по путям.
#[derive(Clone, Default)]
struct Hits(Arc<Mutex<Vec<String>>>);

impl Hits {
    fn of(&self, path: &str) -> usize {
        self.0.lock().unwrap().iter().filter(|p| *p == path).count()
    }
}

/// Локальный сервер BingX. Возвращает: адрес, план ответов, счётчики.
async fn serve() -> (String, Plan, Hits) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    let plan: Plan = Default::default();
    let hits = Hits::default();
    let (p, h) = (plan.clone(), hits.clone());
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            let (p, h) = (p.clone(), h.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16 * 1024];
                let k = s.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..k]).to_string();
                let path = head
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .split('?')
                    .next()
                    .unwrap_or("")
                    .to_string();
                h.0.lock().unwrap().push(path.clone());
                let rule = p
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(x, _, _)| *x == path)
                    .copied();
                let (code, extra, body) = match rule {
                    Some((_, st, ra)) => (
                        st,
                        ra.map(|s| format!("retry-after: {s}\r\n"))
                            .unwrap_or_default(),
                        r#"{"code":100410,"msg":"rate limit"}"#.to_string(),
                    ),
                    None if path == ORDER => (200, String::new(), ORDER_DOC.to_string()),
                    None if path == DEPTH || path == SYMBOLS => {
                        (200, String::new(), DEPTH_DOC.to_string())
                    }
                    None => (404, String::new(), "{}".to_string()),
                };
                let r = format!(
                    "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(r.as_bytes()).await;
            });
        }
    });
    (addr, plan, hits)
}

/// Окружение адаптера BingX на SQLite во временной папке, ключи — в хранилище секретов.
async fn start() -> (Ctx, Store, Plan, Hits) {
    let d = std::env::temp_dir().join(format!("arb-bingx-429-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
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
        raw: RawLog::start(d.join("raw"), settings.clone()),
        taps: Default::default(),
    };
    (ctx, settings, plan, hits)
}

/// Ордер — подписанный POST, как trade.rs.
async fn order(ctx: &Ctx) -> arb::gateway::errors::GatewayResult<Value> {
    let p = vec![("symbol", "NEMS-USDT".to_string())];
    rest::signed(
        ctx,
        Method::POST,
        ORDER,
        p,
        arb::venues::bingx::errors::Op::Trade,
    )
    .await
}

/// Задать или снять venue.rate_limit_pause_ms [bingx].
async fn pause_ms(s: &Store, v: Option<u64>) {
    let sc = Scope::Venue("bingx".into());
    s.set("venue.rate_limit_pause_ms", sc, v.map(|x| json!(x)), "тест")
        .await
        .unwrap();
}

/// Сколько записей журнала вида venue.rate_limit с уровнем.
async fn logged(ctx: &Ctx, level: Level) -> usize {
    let all = ctx.journal.list(500, None).await.unwrap();
    all.iter()
        .filter(|e| e.kind == "venue.rate_limit" && e.level == level)
        .count()
}

#[tokio::test]
async fn pause_429_endpoint_418_venue() {
    let (ctx, s, plan, hits) = start().await;
    let (tx, mut rx) = mpsc::channel(8);
    rate_pause::set_out(tx);
    plan.lock().unwrap().push((DEPTH, 429, None));

    // Настройка не задана — паузы нет (как было), Warn в журнале.
    let e = rest::public(&ctx, DEPTH, vec![]).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::RateLimited, "{}", e.message);
    assert!(e.message.contains("не задана"), "{}", e.message);
    rest::public(&ctx, DEPTH, vec![]).await.unwrap_err();
    assert_eq!(hits.of(DEPTH), 2, "без настройки второй запрос ушёл");
    assert_eq!(logged(&ctx, Level::Warn).await, 2);

    // Задана — 429 стакана ставит паузу только стакану; ордер проходит.
    pause_ms(&s, Some(1500)).await;
    let e = rest::public(&ctx, DEPTH, vec![]).await.unwrap_err();
    assert!(
        e.message.contains("пауза эндпоинта 1500 мс"),
        "{}",
        e.message
    );
    assert_eq!(hits.of(DEPTH), 3);
    let e = rest::public(&ctx, DEPTH, vec![]).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::RateLimited);
    assert!(
        e.message.contains("пауза после HTTP 429") && e.message.contains("ещё"),
        "{}",
        e.message
    );
    assert_eq!(hits.of(DEPTH), 3, "во время паузы запрос не ушёл");
    assert!(rate_pause::pause_left_ms(DEPTH) > 0);
    assert!(
        rate_pause::pause_left_ms(ORDER) <= 0,
        "пауза — только эндпоинта"
    );
    let v = order(&ctx).await.unwrap();
    assert_eq!(v["status"], "PENDING");
    assert_eq!(hits.of(ORDER), 1);
    rest::public(&ctx, SYMBOLS, vec![]).await.unwrap();

    // Retry-After в ответе — длительность по нему.
    plan.lock().unwrap().push((SYMBOLS, 429, Some(1)));
    let e = rest::public(&ctx, SYMBOLS, vec![]).await.unwrap_err();
    assert!(e.message.contains("1000 мс (Retry-After)"), "{}", e.message);

    // Паузы кончились — стакан снова идёт (уже с 200).
    tokio::time::sleep(Duration::from_millis(1600)).await;
    plan.lock().unwrap().clear();
    rest::public(&ctx, DEPTH, vec![]).await.unwrap();
    assert_eq!(hits.of(DEPTH), 4);

    // 418 — пауза всей биржи: ордер не уходит; Error в журнале; событие ядру.
    plan.lock().unwrap().push((DEPTH, 418, None));
    let e = rest::public(&ctx, DEPTH, vec![]).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::RateLimited);
    assert!(
        e.message.contains("пауза всей биржи 1500 мс"),
        "{}",
        e.message
    );
    let e = order(&ctx).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::RateLimited);
    assert!(
        e.message.contains("пауза всей биржи после HTTP 418"),
        "{}",
        e.message
    );
    assert_eq!(hits.of(ORDER), 1, "ордер во время паузы биржи не ушёл");
    assert_eq!(logged(&ctx, Level::Error).await, 1);
    match rx.try_recv().unwrap() {
        MarketEvent::Notice { code, text, .. } => {
            assert_eq!(code, rate_pause::EVENT);
            assert!(text.contains("418"), "{text}");
        }
        _ => panic!("ждали Notice"),
    }

    // Пауза биржи кончилась — ордер проходит.
    tokio::time::sleep(Duration::from_millis(1600)).await;
    order(&ctx).await.unwrap();
    assert_eq!(hits.of(ORDER), 2);
}
