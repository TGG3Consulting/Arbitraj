//! Файл: tests/core_exec_book_238.rs
//! Что делает: решение 238 на живом ядре (настоящая SQLite во временной папке) с настоящим адаптером биржи XT.com,
//! который ходит по HTTP в локальный сервер с записанными ответами (моков логики нет):
//! - справочник пар — живой ответ GET /v4/public/symbol от 06.11.2026 (tests/data/xt_live_2026-11-06/symbol_pepe.json:
//!   btc_usdt, takerFeeRate — из ответа);
//! - стакан REST — пример ответа GET /v4/public/depth из официальной документации XT (xt4-api, _market/3depth.md;
//!   tests/data/rest_book_2026-11-09/xt_depth_doc.json): bids 200 × 0.996, 100 × 0.001, 20 × 10.
//!
//! Стакана потока у пары нет (подписки нет) — адаптер отвечает «устарел» (StaleData), как MEXC в живом случае XTER
//! 08.11 (кадр пары 32 с, venue.book_stale_ms = 5000). Проверяется:
//! - котировка исполнения (trade_ops::sell_value) и худшая цена ордера (worst_order) — по ОДНОМУ снимку REST: один
//!   запрос стакана на оба шага, цена ордера — самый глубокий уровень этого снимка, источник — REST;
//! - кейс XTER целиком (sell_or_stuck): строка хронологии «… по снимку стакана REST …», ни одной строки «… худшая
//!   цена ордера не считается — повтор через режим застрявшей монеты», ни одного пустого повтора (unsent_retries = 0);
//!   ордер дошёл до адаптера (здесь не задана venue.order_time_in_force: отказ до отправки без кода безопасного
//!   повтора — алерт человеку);
//! - REST не ответил — один повтор через exec.rest_book_retry_ms, затем ошибка с причиной без кода безопасного
//!   повтора (не StaleData: продажа — алертом, а не повторами); пауза не задана — без повтора;
//! - снимок REST старше venue.book_stale_ms — не используется (новый запрос: одна мера свежести);
//! - ревью п. 1: ордер ушёл к адаптеру — снимок забыт, следующая попытка — по свежему стакану;
//! - ревью п. 2: не задана venue.book_stale_ms — застрявшая монета («Порог убытка», «Перегон») сразу остановлена;
//! - ревью п. 3: первая попытка ордера — по стакану котировки (тот же снимок), повтор — по свежему;
//! - ревью п. 4: неудача REST запоминается на exec.rest_book_retry_ms (или venue.rate_limit_pause_ms) — в окне без
//!   запроса.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::venues::xt, arb::settings, arb::storage, arb::gateway.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::types::{Asset, Direction, Side, VenueId};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

/// Счётчик запросов стакана и выключатель «REST не отвечает» локального сервера.
#[derive(Clone, Default)]
struct Server {
    depth: Arc<AtomicUsize>,
    fail: Arc<AtomicBool>,
}

/// Тело ответа XT: {"rc": 0, "mc": "SUCCESS", "ma": [], "result": …} (README tests/data/xt_live_2026-11-06).
fn symbols_body() -> String {
    let t = std::fs::read_to_string("tests/data/xt_live_2026-11-06/symbol_pepe.json").unwrap();
    let result: Value = serde_json::from_str(&t).unwrap();
    json!({"rc": 0, "mc": "SUCCESS", "ma": [], "result": result}).to_string()
}

/// Локальный сервер с записанными ответами. Возвращает: адрес и счётчики.
async fn serve() -> (String, Server) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    let srv = Server::default();
    let depth =
        std::fs::read_to_string("tests/data/rest_book_2026-11-09/xt_depth_doc.json").unwrap();
    let symbols = symbols_body();
    let s = srv.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut c, _)) = l.accept().await else {
                return;
            };
            let (depth, symbols, s) = (depth.clone(), symbols.clone(), s.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16 * 1024];
                let n = c.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                let (code, body) = if path.starts_with("/v4/public/depth") {
                    s.depth.fetch_add(1, Ordering::SeqCst);
                    if s.fail.load(Ordering::SeqCst) {
                        ("500 Internal Server Error", "{}".to_string())
                    } else {
                        ("200 OK", depth)
                    }
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
    (addr, srv)
}

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-238-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Ядро с адаптером XT на базе в папке. Возвращает: ядро, настройки, журнал, счётчики сервера.
async fn start(d: &Path) -> (Core, Store, Journal, Server) {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let (url, srv) = serve().await;
    let vault = arb::secrets::Vault::locked(d.join("secrets.enc"));
    let g = |n: &str, v: Value| (n.to_string(), Scope::Global, v);
    let x = |n: &str, v: Value| (n.to_string(), Scope::Venue("xt".into()), v);
    for (n, sc, v) in [
        g("http.timeout_ms", json!(5000)),
        g("http.body_idle_ms", json!(5000)),
        g("http.total_ms", json!(10000)),
        g("http.max_body_bytes", json!(10_000_000)),
        x("venue.rest_url", json!(url)),
        x("venue.quote_asset", json!("USDT")),
        x("venue.symbols_refresh_s", json!(3600)),
        x("feed.stale_after_ms", json!(30000)),
        // Как у MEXC в живом случае XTER: 5000 мс.
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
        settings.set(&n, sc, Some(v), "тест").await.unwrap();
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
    let core = Core::start(Deps {
        gw,
        settings: settings.clone(),
        journal: journal.clone(),
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    (core, settings, journal, srv)
}

fn btc() -> Asset {
    Asset {
        ticker: "BTC".into(),
        bsc_contract: None,
    }
}

fn xt() -> VenueId {
    VenueId("xt".into())
}

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

#[tokio::test]
async fn quote_and_order_use_one_rest_snapshot() {
    let d = dir("one");
    let (core, _, _, srv) = start(&d).await;
    // 1 BTC: 0.996 по 200 + 0.001 по 100 + 0.003 по 20 — самый глубокий уровень 20.
    let qty = dec("1");
    let (_, q) = arb::core::trade_ops::sell_value(&core, None, &xt(), &btc(), qty)
        .await
        .expect("котировка исполнения по снимку REST");
    assert_eq!(q.fillable_qty, qty);
    let avg =
        (dec("200") * dec("0.996") + dec("100") * dec("0.001") + dec("20") * dec("0.003")) / qty;
    assert_eq!(q.avg_price, avg);
    let w = arb::core::worst::worst_order(&core, &xt(), &btc(), Side::Sell, qty, q.avg_price)
        .await
        .expect("худшая цена ордера по тому же снимку");
    assert!(w.rest, "источник — снимок REST");
    assert_eq!(w.price, dec("20"));
    assert_eq!(
        srv.depth.load(Ordering::SeqCst),
        1,
        "один запрос стакана на котировку и ордер"
    );
}

#[tokio::test]
async fn xter_case_sells_by_rest_without_empty_retries() {
    let d = dir("xter");
    let (core, s, _, srv) = start(&d).await;
    s.set(
        "stuck.unsent_retries_max",
        Scope::Global,
        Some(json!(5)),
        "тест",
    )
    .await
    .unwrap();
    let dir = Direction {
        asset: btc(),
        buy_on: VenueId("bsc_dex".into()),
        sell_on: xt(),
    };
    let mut deal = Deal::new(
        "20261109000000000BTC".into(),
        dir,
        json!(null),
        Decimal::ZERO,
    );
    deal.at = xt();
    deal.qty = dec("1");
    deal.step = Step::Selling;
    arb::core::sell::sell_or_stuck(&core, deal).await;
    let x = core
        .deals()
        .into_iter()
        .find(|d| d.id == "20261109000000000BTC")
        .unwrap();
    let lines: Vec<&str> = x.timeline.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        lines
            .iter()
            .any(|t| t.starts_with("Котировка продажи на xt")
                && t.contains("худшая цена ордера 20.0000 по снимку стакана REST")),
        "{lines:#?}"
    );
    // Живой случай XTER: «стакан XTER устарел … — худшая цена ордера не считается — повтор через режим застрявшей монеты».
    assert!(
        !lines
            .iter()
            .any(|t| t.contains("не считается") || t.contains("повтор через режим застрявшей")),
        "{lines:#?}"
    );
    assert_eq!(x.unsent_retries, 0, "пустых повторов нет");
    assert!(
        lines
            .iter()
            .any(|t| t.contains("venue.order_time_in_force")),
        "{lines:#?}"
    );
    // Котировка и ордер — один снимок (1 запрос); ревью п. 1: ордер ушёл к адаптеру — снимок забыт, оценка убытка
    // алерта (sell_unsent::alert → deal_value) — по свежему стакану (2-й запрос).
    assert_eq!(srv.depth.load(Ordering::SeqCst), 2);
}

/// Решение 238 (ревью, п. 1): сценарий ревью — ордер по худшей цене снимка S ушёл к адаптеру и не исполнился (здесь —
/// отказ адаптера: не задана venue.order_time_in_force) — следующая котировка и худшая цена (повтор, «продажа здесь»)
/// не берут тот же S, а запрашивают стакан заново.
#[tokio::test]
async fn snapshot_forgotten_after_order() {
    let d = dir("forget");
    let (core, _, _, srv) = start(&d).await;
    let qty = dec("1");
    let (_, q) = arb::core::trade_ops::sell_value(&core, None, &xt(), &btc(), qty)
        .await
        .unwrap();
    assert_eq!(srv.depth.load(Ordering::SeqCst), 1);
    let e = arb::core::trade_ops::trade(
        &core,
        "20261109000000001BTC",
        &xt(),
        &btc(),
        Side::Sell,
        qty,
        None,
        q.avg_price,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(
        e.message.contains("venue.order_time_in_force"),
        "{}",
        e.message
    );
    // Худшая цена ордера — по тому же снимку, что котировка (запроса не было).
    assert_eq!(srv.depth.load(Ordering::SeqCst), 1);
    let w = arb::core::worst::worst_order(&core, &xt(), &btc(), Side::Sell, qty, q.avg_price)
        .await
        .unwrap();
    assert!(w.rest);
    assert_eq!(
        srv.depth.load(Ordering::SeqCst),
        2,
        "после ордера снимок S забыт — новый запрос стакана"
    );
    // Новый снимок снова общий для котировки и ордера.
    arb::core::trade_ops::sell_value(&core, None, &xt(), &btc(), qty)
        .await
        .unwrap();
    assert_eq!(srv.depth.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn rest_down_one_retry_then_error_with_reason() {
    let d = dir("down");
    let (core, s, _, srv) = start(&d).await;
    srv.fail.store(true, Ordering::SeqCst);
    // Пауза не задана — без повтора, причина в тексте.
    let e = arb::core::worst::worst_order(&core, &xt(), &btc(), Side::Sell, dec("1"), dec("100"))
        .await
        .unwrap_err();
    assert!(e.message.contains("стакана нет"), "{}", e.message);
    assert!(
        e.message.contains("не задана exec.rest_book_retry_ms"),
        "{}",
        e.message
    );
    assert!(
        !e.retry_safe(),
        "не безопасный повтор: алерт, а не повторы по тому же стакану"
    );
    // Ревью п. 4: окна неудачи нет (не задана ни exec.rest_book_retry_ms, ни venue.rate_limit_pause_ms) — сказано.
    assert!(
        e.message.contains("частота запросов REST не ограничена"),
        "{}",
        e.message
    );
    assert_eq!(srv.depth.load(Ordering::SeqCst), 1);
    // Пауза задана — ровно один повтор.
    s.set(
        "exec.rest_book_retry_ms",
        Scope::Venue("xt".into()),
        Some(json!(30)),
        "тест",
    )
    .await
    .unwrap();
    // Неудача без окна не запоминалась — запрос идёт сразу.
    let e = arb::core::worst::worst_order(&core, &xt(), &btc(), Side::Sell, dec("1"), dec("100"))
        .await
        .unwrap_err();
    assert!(e.message.contains("повтор через 30 мс"), "{}", e.message);
    assert_eq!(e.code, None);
    assert_eq!(srv.depth.load(Ordering::SeqCst), 3);
    // REST ожил — снимок берётся (после окна прошлой неудачи 30 мс — ревью п. 4).
    srv.fail.store(false, Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    let w = arb::core::worst::worst_order(&core, &xt(), &btc(), Side::Sell, dec("1"), dec("100"))
        .await
        .unwrap();
    assert!(w.rest);
}

#[tokio::test]
async fn old_rest_snapshot_is_not_reused() {
    let d = dir("old");
    let (core, s, _, srv) = start(&d).await;
    s.set(
        "venue.book_stale_ms",
        Scope::Venue("xt".into()),
        Some(json!(50)),
        "тест",
    )
    .await
    .unwrap();
    let w = |c: Core| async move {
        arb::core::worst::worst_order(&c, &xt(), &btc(), Side::Sell, dec("1"), dec("100"))
            .await
            .unwrap()
    };
    w(core.clone()).await;
    assert_eq!(srv.depth.load(Ordering::SeqCst), 1);
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    w(core.clone()).await;
    assert_eq!(
        srv.depth.load(Ordering::SeqCst),
        2,
        "снимок старше venue.book_stale_ms — новый запрос"
    );
}

/// Сделка на xt: 1 BTC, куплено на bsc_dex (для сценариев застрявшей монеты).
fn stuck_deal(id: &str) -> Deal {
    let dir = Direction {
        asset: btc(),
        buy_on: VenueId("bsc_dex".into()),
        sell_on: xt(),
    };
    let mut deal = Deal::new(id.into(), dir, json!(null), Decimal::ZERO);
    deal.at = xt();
    deal.qty = dec("1");
    deal
}

/// Решение 238 (ревью, п. 2): сценарий ревью — у биржи не задана venue.book_stale_ms, монета в режиме застрявшей
/// монеты «Порог убытка» и «Перегон»: раньше «Порог убытка» ждал тика без предела (нет алерта), теперь сделка сразу
/// остановлена с причиной «не задана настройка …». Тиков монеты в тесте нет — ожидание тика повисло бы (timeout).
#[tokio::test]
async fn stuck_without_book_stale_ms_halts_at_once() {
    use arb::core::stuck_reason::StuckReason;
    let d = dir("noconf");
    let (core, s, _, srv) = start(&d).await;
    let xt_scope = || Scope::Venue("xt".into());
    s.set("venue.book_stale_ms", xt_scope(), None, "тест")
        .await
        .unwrap();
    for (n, v) in [("stuck.max_loss_pct", 1), ("stuck.max_relays", 3)] {
        s.set(n, Scope::Global, Some(json!(v)), "тест")
            .await
            .unwrap();
    }
    for (mode, id) in [
        ("loss_limit", "20261109000000002BTC"),
        ("relay", "20261109000000003BTC"),
    ] {
        s.set("stuck.mode", Scope::Global, Some(json!(mode)), "тест")
            .await
            .unwrap();
        let run = arb::core::stuck::handle_as_is(&core, stuck_deal(id), StuckReason::Unprofitable);
        tokio::time::timeout(std::time::Duration::from_secs(5), run)
            .await
            .unwrap_or_else(|_| panic!("{mode}: ждёт тика без предела"));
        let x = core.deals().into_iter().find(|d| d.id == id).unwrap();
        assert_eq!(x.step, Step::Halted, "{mode}");
        let lines: Vec<&str> = x.timeline.iter().map(|(_, t)| t.as_str()).collect();
        assert!(
            lines
                .iter()
                .any(|t| t.contains("не задана настройка") && t.contains("venue.book_stale_ms")),
            "{mode}: {lines:#?}"
        );
    }
    assert_eq!(
        srv.depth.load(Ordering::SeqCst),
        0,
        "стакан не запрашивался"
    );
}

/// Решение 238 (ревью, п. 3): сценарий ревью — котировка продажи (порог убытка) по снимку S1, а лимит IOC первой
/// попытки — по новому снимку S2. Здесь S1 к моменту ордера старше venue.book_stale_ms (50 мс): без стакана котировки
/// худшая цена взяла бы новый снимок (2-й запрос); со стаканом котировки — тот же S1, запроса нет.
#[tokio::test]
async fn first_order_uses_quote_book() {
    let d = dir("same");
    let (core, s, _, srv) = start(&d).await;
    s.set(
        "venue.book_stale_ms",
        Scope::Venue("xt".into()),
        Some(json!(50)),
        "тест",
    )
    .await
    .unwrap();
    let qty = dec("1");
    let (_, q, book) = arb::core::trade_ops::sell_quote(&core, None, &xt(), &btc(), qty)
        .await
        .unwrap();
    let book = book.expect("у биржи котировка отдаёт стакан");
    assert!(book.rest);
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    let order = |b: Option<arb::core::exec_book::Book>| {
        let (core, q) = (core.clone(), q.clone());
        async move {
            let mut seen = None;
            let r = arb::core::trade_ops::trade_seen(
                &core,
                "20261109000000004BTC",
                &xt(),
                &btc(),
                Side::Sell,
                qty,
                None,
                q.avg_price,
                None,
                b,
                &mut seen,
                None,
            )
            .await;
            (r, seen)
        }
    };
    let (r, seen) = order(Some(book)).await;
    // Ордер дошёл до адаптера (отказ: не задана venue.order_time_in_force) — худшая цена посчитана по S1.
    assert!(r.unwrap_err().message.contains("venue.order_time_in_force"));
    let w = seen.expect("худшая цена посчитана");
    assert!(w.rest);
    assert_eq!(w.price, dec("20"));
    assert_eq!(
        srv.depth.load(Ordering::SeqCst),
        1,
        "котировка и ордер — один снимок"
    );
    // Без стакана котировки (повтор) — свежий снимок.
    let (_, seen) = order(None).await;
    assert!(seen.unwrap().rest);
    assert_eq!(srv.depth.load(Ordering::SeqCst), 2);
}

/// Решение 238 (ревью, п. 4): сценарий ревью — REST стакана отказывает постоянно, а застрявшая монета («Ожидание»,
/// «Порог убытка») котирует на каждый тик монеты. Неудача запоминается на exec.rest_book_retry_ms (не задана —
/// venue.rate_limit_pause_ms): в окне — сразу ошибка с причиной, без запроса; после окна — снова запрос.
#[tokio::test]
async fn rest_failure_is_remembered_for_window() {
    let d = dir("window");
    let (core, s, _, srv) = start(&d).await;
    srv.fail.store(true, Ordering::SeqCst);
    let ask = |c: Core| async move {
        arb::core::worst::worst_order(&c, &xt(), &btc(), Side::Sell, dec("1"), dec("100"))
            .await
            .unwrap_err()
    };
    let n = || srv.depth.load(Ordering::SeqCst);
    let set = |name: &'static str, v: Option<Value>| {
        let s = s.clone();
        async move {
            s.set(name, Scope::Venue("xt".into()), v, "тест")
                .await
                .unwrap()
        }
    };
    // Окно — пауза повтора REST (200 мс): запрос + повтор, затем в окне — ни одного запроса.
    set("exec.rest_book_retry_ms", Some(json!(200))).await;
    let e = ask(core.clone()).await;
    assert!(
        e.message.contains("не раньше чем через 200 мс"),
        "{}",
        e.message
    );
    assert_eq!(n(), 2);
    for _ in 0..5 {
        let e = ask(core.clone()).await;
        assert!(e.message.contains("не запрашивался"), "{}", e.message);
        assert!(
            e.message.contains("окно 200 мс (exec.rest_book_retry_ms)"),
            "{}",
            e.message
        );
        assert!(!e.retry_safe(), "алерт, а не повтор по тому же стакану");
    }
    assert_eq!(n(), 2, "в окне неудачи REST не запрашивается");
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    ask(core.clone()).await;
    assert_eq!(n(), 4, "окно прошло — снова запрос и повтор");
    // Без exec.rest_book_retry_ms — окно venue.rate_limit_pause_ms (повтора нет: один запрос).
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    set("exec.rest_book_retry_ms", None).await;
    set("venue.rate_limit_pause_ms", Some(json!(200))).await;
    ask(core.clone()).await;
    assert_eq!(n(), 5);
    let e = ask(core.clone()).await;
    assert!(
        e.message.contains("(venue.rate_limit_pause_ms)"),
        "{}",
        e.message
    );
    assert_eq!(n(), 5);
    // REST ожил: после окна снимок берётся, неудача снята.
    srv.fail.store(false, Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let w = arb::core::worst::worst_order(&core, &xt(), &btc(), Side::Sell, dec("1"), dec("100"))
        .await
        .unwrap();
    assert!(w.rest);
    assert_eq!(n(), 6);
}
