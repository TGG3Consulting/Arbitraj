//! Файл: tests/core_market_sell_268.rs
//! Что делает: решение 268 (Тигран 14.11.2026) на живом ядре (настоящая SQLite во временной папке) с настоящим
//! адаптером биржи XT.com, который ходит по HTTP в локальный сервер (моков логики нет; как tests/core_exec_book_238.rs):
//! - справочник пар — живой ответ GET /v4/public/symbol от 06.11.2026 (tests/data/xt_live_2026-11-06/symbol_pepe.json:
//!   btc_usdt, orderTypes [LIMIT, MARKET], timeInForces [GTC, IOC]);
//! - стакан REST — пример ответа GET /v4/public/depth из официальной документации XT
//!   (tests/data/rest_book_2026-11-09/xt_depth_doc.json); «стакан хуже» — тот же пример с ценами × 0.9 (собран в тесте);
//! - ОТВЕТЫ НА ОРДЕРА СОБРАНЫ ПО ОФИЦИАЛЬНОЙ ДОКУМЕНТАЦИИ, А НЕ ЗАПИСАНЫ (живых записей в tests/data нет — заменить
//!   записанными после первого живого случая, docs/TASKS.md): отказы — конверт XT {rc, mc, ma} (venues/xt/reply.rs) с
//!   кодами таблицы https://doc.xt.com/docs/spot/Access%20Description/ResponseCode: на ЛИМИТНЫЙ ордер — ORDER_F0501
//!   («Trigger PROTECTION_LIMIT Filter» → PriceOutOfRange), на РЫНОЧНЫЙ — ORDER_F0601 («Trigger PROTECTION_MARKET
//!   Filter» → обычный отказ); успех постановки — result {orderId} (Submit order); состояние ордера — поля Query single
//!   (https://doc.xt.com/docs/spot/Order/QuerySingle): state, origQty, executedQty, avgPrice, tradeQuote, fee,
//!   feeCurrency; HTTP 500 на постановку — «итог неизвестен».
//!
//! Проверяется путь целиком:
//! - обычная продажа, выгодно: ровно один рыночный ордер (type MARKET, IOC, quantity, без price, свой clientOrderId);
//!   исполнен полностью — сделка закрыта, выручка учтена; исполнен частично — остаток остаётся в сделке и идёт дальше
//!   существующим путём; отклонён — алерт человеку, третьего ордера нет; правило шлюза «Повторить» рыночный ордер не
//!   повторяет; «итог неизвестен» у лимитного — рыночный не уходит;
//! - обычная продажа, на свежей котировке невыгодно: режим застрявшей монеты (раз на площадку); режим «Порог убытка»
//!   решил продать, его ордер упёрся в коридор, убыток по свежей котировке в пороге — рыночный ордер; порог режима
//!   превышен уже при входе в режим — алерт без ордеров;
//! - режим застрявшей монеты (Тигран 14.11: «тот же механизм»): продажа здесь в «Перегоне» (relay_here) и «Порог
//!   убытка» (loss_limit) — убыток по свежей котировке в пороге → ровно один MARKET; выше порога → алерт, MARKET не
//!   уходит; «Ожидание» истекло (без порога) → алерт, MARKET не уходит;
//! - кнопка человека — рыночный ордер без проверки условия; повторный отказ без нового нажатия — алерт;
//! - настройка не задана — как до решения: один лимитный ордер, алерт, строк решения 268 нет.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::venues::xt, arb::settings, arb::storage, arb::gateway, arb::secrets.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::types::{Asset, Direction, VenueId};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::secrets::crypto::KdfParams;
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
use zeroize::Zeroizing;

const KDF: KdfParams = KdfParams {
    memory_kib: 64,
    iterations: 1,
    parallelism: 1,
};

/// Что отвечает сервер и что до него дошло.
#[derive(Default)]
struct Plan {
    /// Тела запросов POST /v4/order по порядку.
    orders: Vec<Value>,
    /// Ответ на рыночный ордер: None — отказ ORDER_F0601; Some(исполнено) — принят (orderId M1) и исполнен на столько.
    market_fill: Option<&'static str>,
    /// Лимитный ордер: true — HTTP 500 («итог неизвестен»); false — отказ ORDER_F0501.
    limit_500: bool,
    /// После скольких принятых ордеров стакан становится хуже (цены × 0.9); None — не меняется.
    worse_after: Option<usize>,
    /// После скольких принятых ордеров стакан по REST не отвечает (HTTP 500); None — отвечает.
    depth_down_after: Option<usize>,
}

type Srv = Arc<Mutex<Plan>>;

fn pw() -> Zeroizing<String> {
    Zeroizing::new("тест".to_string())
}

/// Тело ответа XT: {"rc": 0, "mc": "SUCCESS", "ma": [], "result": …} (README tests/data/xt_live_2026-11-06).
fn symbols_body() -> String {
    let t = std::fs::read_to_string("tests/data/xt_live_2026-11-06/symbol_pepe.json").unwrap();
    let result: Value = serde_json::from_str(&t).unwrap();
    ok(result)
}

fn ok(result: Value) -> String {
    json!({"rc": 0, "mc": "SUCCESS", "ma": [], "result": result}).to_string()
}

/// Стакан примера документации; worse — цены покупателей × 0.9.
fn depth(worse: bool) -> String {
    let t = std::fs::read_to_string("tests/data/rest_book_2026-11-09/xt_depth_doc.json").unwrap();
    if !worse {
        return t;
    }
    let mut v: Value = serde_json::from_str(&t).unwrap();
    for b in v["result"]["bids"].as_array_mut().unwrap() {
        let p = Decimal::from_str(b[0].as_str().unwrap()).unwrap() * Decimal::new(9, 1);
        b[0] = json!(p.to_string());
    }
    v.to_string()
}

/// Запрос целиком: заголовки и тело по content-length. Возвращает: (метод, путь, тело).
async fn request(c: &mut tokio::net::TcpStream) -> (String, String, String) {
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        let n = c.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf).to_string();
        let Some(cut) = text.find("\r\n\r\n") else {
            continue;
        };
        let need = text[..cut]
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.eq_ignore_ascii_case("content-length")
                    .then(|| v.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);
        if buf.len() >= cut + 4 + need {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let method = text.split_whitespace().next().unwrap_or("").to_string();
    let path = text.split_whitespace().nth(1).unwrap_or("").to_string();
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (method, path, body)
}

/// Состояние рыночного ордера M1 (поля Query single): исполнено `filled` из 1 по средней 199 USDT.
fn market_state(filled: &str) -> Value {
    let f = Decimal::from_str(filled).unwrap();
    let state = if f >= Decimal::ONE {
        "FILLED"
    } else {
        "CANCELED"
    };
    json!({"symbol": "btc_usdt", "orderId": "M1", "state": state, "type": "MARKET", "side": "SELL",
           "origQty": "1", "executedQty": filled, "leavingQty": (Decimal::ONE - f).to_string(),
           "avgPrice": "199", "tradeQuote": (f * Decimal::from(199)).to_string(),
           "fee": "0.1", "feeCurrency": "usdt"})
}

/// Локальный сервер (см. шапку). Возвращает: адрес и план.
async fn serve() -> (String, Srv) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    let srv: Srv = Default::default();
    let symbols = symbols_body();
    let s = srv.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut c, _)) = l.accept().await else {
                return;
            };
            let (symbols, s) = (symbols.clone(), s.clone());
            tokio::spawn(async move {
                let (method, path, body) = request(&mut c).await;
                let (code, body) = if path.starts_with("/v4/public/depth") {
                    let p = s.lock().unwrap();
                    let worse = p.worse_after.is_some_and(|n| p.orders.len() >= n);
                    if p.depth_down_after.is_some_and(|n| p.orders.len() >= n) {
                        ("500 Internal Server Error", "{}".to_string())
                    } else {
                        ("200 OK", depth(worse))
                    }
                } else if path.starts_with("/v4/public/symbol") {
                    ("200 OK", symbols)
                } else if path.starts_with("/v4/order") && method == "POST" {
                    let o: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    let mut p = s.lock().unwrap();
                    let market = o["type"] == "MARKET";
                    p.orders.push(o);
                    let refuse = |mc: &str| json!({"rc": 1, "mc": mc, "ma": [], "result": null});
                    match (market, p.market_fill, p.limit_500) {
                        (true, Some(_), _) => ("200 OK", ok(json!({"orderId": "M1"}))),
                        (true, None, _) => ("200 OK", refuse("ORDER_F0601").to_string()),
                        (false, _, true) => ("500 Internal Server Error", "{}".to_string()),
                        (false, _, false) => ("200 OK", refuse("ORDER_F0501").to_string()),
                    }
                } else if path.starts_with("/v4/order") && path.contains("orderId=M1") {
                    let fill = s.lock().unwrap().market_fill.unwrap_or("0");
                    ("200 OK", ok(market_state(fill)))
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
    let d = std::env::temp_dir().join(format!("arb-268-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Ядро с адаптером XT (ключи заданы, хранилище открыто). on — значение sell.market_on_price_range площадки (None —
/// не задавать). Возвращает: ядро, настройки и план сервера.
async fn start(d: &Path, on: Option<bool>) -> (Core, Store, Srv) {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let (url, srv) = serve().await;
    let vault = arb::secrets::Vault::locked(d.join("secrets.enc"));
    vault.create(pw(), KDF).await.unwrap();
    for n in ["xt.api_key", "xt.api_secret"] {
        let v = Some(Zeroizing::new("k".to_string()));
        vault.change(pw(), n, v, KDF).await.unwrap();
    }
    let g = |n: &str, v: Value| (n.to_string(), Scope::Global, v);
    let x = |n: &str, v: Value| (n.to_string(), Scope::Venue("xt".into()), v);
    let mut list = vec![
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
        x("venue.order_time_in_force", json!("IOC")),
        x("venue.order_poll_ms", json!(10)),
        x("venue.order_final_wait_ms", json!(50)),
        x("venue.recv_window_ms", json!(5000)),
    ];
    if let Some(v) = on {
        list.push(x(arb::core::sell_range::ON, json!(v)));
    }
    for (n, sc, v) in list {
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
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    (core, settings, srv)
}

async fn set(s: &Store, name: &str, v: Value) {
    s.set(name, Scope::Global, Some(v), "тест").await.unwrap();
}

/// Стек потока теста. В отладочной сборке путь продажи с настоящим адаптером (вложенные будущие sell → ордер → алерт)
/// не помещается в стек тестового потока по умолчанию (2 МиБ) — и без решения 268: замер этим же стендом на ef2c8a4
/// (до решения; порог RUST_MIN_STACK) — кнопка «Продать сейчас» с отказом лимитного ордера ≈ 2.4 МиБ, авто-продажа
/// ≈ 1.3 МиБ. Путь с веткой решения 268 глубже (sell → ветка → рыночный ордер; после неё — режим застрявшей монеты
/// или алерт). Параметр теста, не настройка бота.
const STACK: usize = 16 * 1024 * 1024;

/// Выполняет тело теста в потоке со стеком STACK на своём однопоточном рантайме.
fn big<F>(body: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let run = move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(body);
    };
    let t = std::thread::Builder::new().stack_size(STACK);
    t.spawn(run).unwrap().join().unwrap();
}

fn xt() -> VenueId {
    VenueId("xt".into())
}

/// Сделка: 1 BTC лежит на xt, куплено на bsc_dex; вложено `invested` USDT (0 — любая продажа выгодна). По стакану
/// примера 1 BTC продаётся примерно за 199 USDT, по «стакану хуже» — примерно за 179.
fn deal(id: &str, invested: &str) -> Deal {
    let dir = Direction {
        asset: Asset {
            ticker: "BTC".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("bsc_dex".into()),
        sell_on: xt(),
    };
    let mut deal = Deal::new(id.into(), dir, json!(null), Decimal::ZERO);
    deal.at = xt();
    deal.qty = Decimal::ONE;
    deal.invested = Decimal::from_str(invested).unwrap();
    deal.step = Step::Selling;
    deal
}

fn saved(core: &Core, id: &str) -> Deal {
    core.deals().into_iter().find(|d| d.id == id).unwrap()
}

fn lines(d: &Deal) -> Vec<String> {
    d.timeline.iter().map(|(_, t)| t.clone()).collect()
}

fn types(o: &Srv) -> Vec<String> {
    o.lock()
        .unwrap()
        .orders
        .iter()
        .map(|x| x["type"].as_str().unwrap_or("?").to_string())
        .collect()
}

const ID: &str = "20261113211300000BTC";
const MARKET_LINE: &str = "продаём рыночным ордером (решение 268)";

#[test]
fn profitable_sale_market_rejected_alerts_without_third_order() {
    big(async {
        let d = dir("market");
        let (core, _, srv) = start(&d, Some(true)).await;
        arb::core::sell::sell_or_stuck(&core, deal(ID, "0")).await;
        assert_eq!(types(&srv), vec!["LIMIT", "MARKET"], "ровно один рыночный");
        let sent = srv.lock().unwrap().orders.clone();
        let (limit, market) = (&sent[0], &sent[1]);
        assert!(limit.get("price").is_some());
        assert_eq!(market["side"], "SELL");
        assert_eq!(market["timeInForce"], "IOC");
        assert_eq!(market["bizType"], "SPOT");
        assert_eq!(market["symbol"], limit["symbol"]);
        assert_eq!(market["quantity"], limit["quantity"], "то же количество");
        for k in ["price", "quoteQty"] {
            assert!(market.get(k).is_none(), "{market}");
        }
        assert_ne!(market["clientOrderId"], limit["clientOrderId"], "свой ID");
        let x = saved(&core, ID);
        let t = lines(&x);
        assert_eq!(x.range_market, vec![xt()]);
        assert!(x.range_stuck.is_empty());
        assert!(
            t.iter().any(|l| l.starts_with(
                "Биржа отклонила лимитный ордер продажи: цена вне разрешённого диапазона (XT, код ORDER_F0501"
            ) && l.contains("снимок REST")
                && l.contains("продажа выгодна")
                && l.ends_with(MARKET_LINE)),
            "{t:#?}"
        );
        // Отказ рыночного ордера — обычный отказ: алерт человеку, монеты на месте, повторов нет.
        assert_eq!(x.step, Step::WaitingHuman, "{t:#?}");
        assert!(
            t.iter()
                .any(|l| l.contains("ORDER_F0601") && l.contains("ждём решения человека")),
            "{t:#?}"
        );
        assert_eq!((x.unsent_retries, x.qty), (0, Decimal::ONE));
    });
}

#[test]
fn market_filled_closes_deal_with_proceeds() {
    big(async {
        let d = dir("filled");
        let (core, _, srv) = start(&d, Some(true)).await;
        srv.lock().unwrap().market_fill = Some("1");
        arb::core::sell::sell_or_stuck(&core, deal(ID, "150")).await;
        assert_eq!(types(&srv), vec!["LIMIT", "MARKET"]);
        let x = saved(&core, ID);
        let t = lines(&x);
        assert_eq!(x.step, Step::Closed, "{t:#?}");
        assert_eq!(x.qty, Decimal::ZERO);
        assert_eq!(x.sells.len(), 1);
        assert_eq!(x.sells[0].venue_ref, "M1");
        assert_eq!(x.sells[0].filled_qty, Decimal::ONE);
        // Выручка — по факту ордера (1 × 199), комиссия 0.1 USDT — в затратах.
        assert_eq!(x.proceeds, Decimal::from(199));
        assert_eq!(x.spent, Decimal::new(1, 1));
        assert!(
            t.iter()
                .any(|l| l.starts_with("Рыночный ордер продажи исполнен: 1 по средней 199")),
            "{t:#?}"
        );
    });
}

#[test]
fn market_partially_filled_keeps_remainder() {
    big(async {
        let d = dir("partial");
        let (core, _, srv) = start(&d, Some(true)).await;
        srv.lock().unwrap().market_fill = Some("0.4");
        arb::core::sell::sell_or_stuck(&core, deal(ID, "150")).await;
        // Остаток идёт существующим путём (режим застрявшей монеты; stuck.mode в тесте не задан — сделка остановлена):
        // второго рыночного ордера нет.
        let n = types(&srv).iter().filter(|t| *t == "MARKET").count();
        assert_eq!(n, 1, "{:?}", types(&srv));
        let x = saved(&core, ID);
        let t = lines(&x);
        assert_eq!(x.sells.len(), 1);
        assert_eq!(x.sells[0].filled_qty, Decimal::new(4, 1));
        assert_eq!(x.qty, Decimal::new(6, 1), "{t:#?}");
        assert_eq!(x.proceeds, Decimal::new(796, 1));
        assert!(
            t.iter().any(|l| l.contains("Продано не всё, остаток 0.6")),
            "{t:#?}"
        );
    });
}

#[test]
fn retry_rule_does_not_repeat_market_order() {
    big(async {
        use arb::gateway::errors::ErrorCode;
        use arb::gateway::rules::{Rule, RuleAction, RuleScope};
        let d = dir("rule");
        let (core, _, srv) = start(&d, Some(true)).await;
        // Правило «Повторить» для OrderRejected: оно же действует для PriceOutOfRange (своего правила нет) — но повтор
        // по этому коду заглушён; отказ рыночного ордера (OrderRejected) правило тоже не повторяет.
        let rule = Rule {
            id: 0,
            event: ErrorCode::OrderRejected,
            condition: None,
            action: RuleAction::Retry {
                attempts: 3,
                pause_ms: 1,
            },
            scope: RuleScope::All,
            enabled: true,
        };
        arb::storage::rules::create(&core.inner.deps.db, rule)
            .await
            .unwrap();
        arb::core::sell::sell_or_stuck(&core, deal(ID, "0")).await;
        assert_eq!(types(&srv), vec!["LIMIT", "MARKET"]);
        assert_eq!(saved(&core, ID).step, Step::WaitingHuman);
    });
}

#[test]
fn limit_outcome_unknown_sends_no_market_order() {
    big(async {
        let d = dir("unknown");
        let (core, _, srv) = start(&d, Some(true)).await;
        srv.lock().unwrap().limit_500 = true;
        arb::core::sell::sell_or_stuck(&core, deal(ID, "0")).await;
        assert_eq!(types(&srv), vec!["LIMIT"], "рыночный не уходит");
        let x = saved(&core, ID);
        let t = lines(&x);
        assert_eq!(x.step, Step::Halted, "{t:#?}");
        assert!(x.range_market.is_empty());
        assert!(
            t.iter().any(|l| l.contains("исход продажи неизвестен")),
            "{t:#?}"
        );
        assert!(!t.iter().any(|l| l.contains("решение 268")), "{t:#?}");
    });
}

/// Обычная продажа: при входе выгодно (≈199 против вложенных 190), после отказа лимитного стакан хуже (≈179) —
/// невыгодно → режим застрявшей монеты (раз на площадку). «Порог убытка» 10%: убыток ≈ 6% в пороге → режим продаёт
/// лимитным, снова отказ по коридору → условие режима на свежей котировке выполнено → рыночный ордер.
#[test]
fn entry_unprofitable_goes_to_mode_then_mode_sells_market() {
    big(async {
        let d = dir("entry-mode");
        let (core, s, srv) = start(&d, Some(true)).await;
        set(&s, "stuck.mode", json!("loss_limit")).await;
        set(&s, "stuck.max_loss_pct", json!("10")).await;
        srv.lock().unwrap().worse_after = Some(1);
        arb::core::sell::sell_or_stuck(&core, deal(ID, "190")).await;
        assert_eq!(types(&srv), vec!["LIMIT", "LIMIT", "MARKET"]);
        let x = saved(&core, ID);
        let t = lines(&x);
        assert_eq!(
            (x.range_stuck.clone(), x.range_market.clone()),
            (vec![xt()], vec![xt()])
        );
        assert!(
            t.iter()
                .any(|l| l.contains("По стакану продажа здесь невыгодна")
                    && l.ends_with("режим застрявшей монеты (решение 268)")),
            "{t:#?}"
        );
        assert!(
            t.iter().any(|l| l
                .contains("условие режима застрявшей монеты выполнено и на свежей котировке")
                && l.contains("не больше порога 10")
                && l.ends_with(MARKET_LINE)),
            "{t:#?}"
        );
        // Рыночный здесь отклонён (ORDER_F0601) — алерт; кругов нет.
        assert_eq!(x.step, Step::WaitingHuman, "{t:#?}");
    });
}

/// То же, но порог режима 2%: убыток ≈ 6% выше порога уже при входе в режим — алерт режима, ордеров больше нет.
#[test]
fn entry_unprofitable_mode_threshold_exceeded_alerts() {
    big(async {
        let d = dir("entry-alert");
        let (core, s, srv) = start(&d, Some(true)).await;
        set(&s, "stuck.mode", json!("loss_limit")).await;
        set(&s, "stuck.max_loss_pct", json!("2")).await;
        srv.lock().unwrap().worse_after = Some(1);
        arb::core::sell::sell_or_stuck(&core, deal(ID, "190")).await;
        assert_eq!(types(&srv), vec!["LIMIT"]);
        let x = saved(&core, ID);
        assert_eq!(x.step, Step::WaitingHuman);
        assert_eq!(x.range_stuck, vec![xt()]);
        assert!(x.range_market.is_empty());
    });
}

/// Режим сам решил продать здесь (убыток ≈ 0.4% при пороге 5%), ордер упёрся в коридор. Стакан прежний — условие
/// режима на свежей котировке выполнено → ровно один MARKET; стакан хуже (убыток ≈ 10%) → алерт, MARKET не уходит.
async fn mode_case(name: &str, relay: bool, worse: bool) -> (Vec<String>, Deal) {
    let d = dir(name);
    let (core, s, srv) = start(&d, Some(true)).await;
    if worse {
        srv.lock().unwrap().worse_after = Some(1);
    }
    let x = deal(ID, "200");
    if relay {
        set(&s, "stuck.relay_max_loss_pct", json!("5")).await;
        arb::core::relay_here::sell_here(&core, x, "перегон не выгоднее").await;
    } else {
        set(&s, "stuck.mode", json!("loss_limit")).await;
        set(&s, "stuck.max_loss_pct", json!("5")).await;
        let why = arb::core::stuck_reason::StuckReason::Unprofitable;
        arb::core::stuck::handle_as_is(&core, x, why).await;
    }
    (types(&srv), saved(&core, ID))
}

#[test]
fn mode_sale_within_threshold_goes_market_once() {
    big(async {
        for (name, relay) in [("relay-ok", true), ("loss-ok", false)] {
            let (sent, x) = mode_case(name, relay, false).await;
            assert_eq!(sent, vec!["LIMIT", "MARKET"], "{name}");
            let t = lines(&x);
            assert_eq!(x.range_market, vec![xt()], "{name}");
            assert!(
                x.range_stuck.is_empty(),
                "{name}: в режим из-за коридора не уходили"
            );
            assert!(
                t.iter().any(|l| l
                    .contains("условие режима застрявшей монеты выполнено и на свежей котировке")
                    && l.contains("не больше порога 5")
                    && l.ends_with(MARKET_LINE)),
                "{name}: {t:#?}"
            );
            assert_eq!(
                x.step,
                Step::WaitingHuman,
                "{name}: рыночный отклонён — алерт"
            );
        }
    });
}

#[test]
fn mode_sale_above_threshold_on_fresh_quote_alerts() {
    big(async {
        for (name, relay) in [("relay-bad", true), ("loss-bad", false)] {
            let (sent, x) = mode_case(name, relay, true).await;
            assert_eq!(sent, vec!["LIMIT"], "{name}: MARKET не уходит");
            let t = lines(&x);
            assert!(
                x.range_market.is_empty() && x.range_stuck.is_empty(),
                "{name}"
            );
            assert!(
                t.iter().any(|l| l.contains("выше порога 5")
                    && l.contains("рыночного ордера нет (решение 268)")),
                "{name}: {t:#?}"
            );
            assert_eq!(x.step, Step::WaitingHuman, "{name}: {t:#?}");
        }
    });
}

/// «Ожидание» истекло: продажа без порога убытка — при отказе по коридору рыночного ордера нет, алерт (как до решения).
#[test]
fn wait_expired_has_no_threshold_and_alerts() {
    big(async {
        let d = dir("wait");
        let (core, s, srv) = start(&d, Some(true)).await;
        set(&s, "stuck.mode", json!("wait")).await;
        set(&s, "stuck.wait_ms", json!(30)).await;
        let why = arb::core::stuck_reason::StuckReason::Unprofitable;
        arb::core::stuck::handle_as_is(&core, deal(ID, "200"), why).await;
        assert_eq!(types(&srv), vec!["LIMIT"]);
        let x = saved(&core, ID);
        let t = lines(&x);
        assert_eq!(x.step, Step::WaitingHuman, "{t:#?}");
        assert!(!t.iter().any(|l| l.contains("решение 268")), "{t:#?}");
    });
}

#[test]
fn button_goes_market_without_condition_and_second_refusal_alerts() {
    big(async {
        let d = dir("button");
        let (core, _, srv) = start(&d, Some(true)).await;
        let run = arb::core::sell::market(&core, deal(ID, "1000000"), arb::core::sell::BUTTON);
        arb::core::risk_count::manual(run).await;
        assert_eq!(types(&srv), vec!["LIMIT", "MARKET"]);
        let x = saved(&core, ID);
        let t = lines(&x);
        assert!(
            t.iter().any(|l| l
                .contains("кнопка человека — без проверки порогов; Убыток при продаже сейчас: ")
                && l.contains("оценка выручки")
                && l.ends_with(MARKET_LINE)),
            "{t:#?}"
        );
        assert_eq!(x.step, Step::WaitingHuman, "{t:#?}");
        // Ещё раз кнопкой без сброса отметок (сброс делает само нажатие — manual.rs): рыночного ордера нет, алерт.
        let mut again = x.clone();
        again.step = Step::Selling;
        let run = arb::core::sell::market(&core, again, arb::core::sell::BUTTON);
        arb::core::risk_count::manual(run).await;
        assert_eq!(types(&srv), vec!["LIMIT", "MARKET", "LIMIT"]);
        let x = saved(&core, ID);
        assert_eq!(x.step, Step::WaitingHuman);
        let t = lines(&x);
        let last = t.last().unwrap();
        assert!(
            last.contains("ORDER_F0501") && last.contains("ждём решения человека"),
            "{t:#?}"
        );
    });
}

/// Кнопка, а свежей котировки после отказа лимитного ордера нет (стакан по REST не отвечает): рыночный ордер всё
/// равно уходит (Тигран 14.11), в хронологии честно «оценки нет: причина».
#[test]
fn button_without_fresh_quote_still_goes_market() {
    big(async {
        let d = dir("button-noq");
        let (core, _, srv) = start(&d, Some(true)).await;
        {
            let mut p = srv.lock().unwrap();
            p.depth_down_after = Some(1);
            p.market_fill = Some("1");
        }
        let run = arb::core::sell::market(&core, deal(ID, "150"), arb::core::sell::BUTTON);
        arb::core::risk_count::manual(run).await;
        assert_eq!(types(&srv), vec!["LIMIT", "MARKET"]);
        let sent = srv.lock().unwrap().orders.clone();
        assert_eq!(sent[1]["quantity"], sent[0]["quantity"]);
        assert!(sent[1].get("price").is_none());
        let x = saved(&core, ID);
        let t = lines(&x);
        assert!(
            t.iter().any(
                |l| l.contains("кнопка человека — без проверки порогов: оценки нет: ")
                    && l.ends_with(MARKET_LINE)
            ),
            "{t:#?}"
        );
        assert_eq!(x.step, Step::Closed, "{t:#?}");
        assert_eq!(x.proceeds, Decimal::from(199));
    });
}

#[test]
fn setting_off_keeps_old_behaviour() {
    big(async {
        for (name, on, button) in [
            ("off", None, false),
            ("no", Some(false), false),
            ("off-b", None, true),
        ] {
            let d = dir(name);
            let (core, _, srv) = start(&d, on).await;
            if button {
                let run = arb::core::sell::market(&core, deal(ID, "0"), arb::core::sell::BUTTON);
                arb::core::risk_count::manual(run).await;
            } else {
                arb::core::sell::sell_or_stuck(&core, deal(ID, "0")).await;
            }
            assert_eq!(types(&srv), vec!["LIMIT"], "{name}");
            let x = saved(&core, ID);
            let t = lines(&x);
            assert!(
                x.range_market.is_empty() && x.range_stuck.is_empty(),
                "{name}"
            );
            assert_eq!(x.step, Step::WaitingHuman, "{name}: {t:#?}");
            assert!(
                !t.iter().any(|l| l.contains("решение 268")),
                "{name}: {t:#?}"
            );
            assert!(
                t.iter()
                    .any(|l| l.contains("ORDER_F0501") && l.contains("ждём решения человека")),
                "{name}: {t:#?}"
            );
        }
    });
}
