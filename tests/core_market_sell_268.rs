//! Файл: tests/core_market_sell_268.rs
//! Что делает: решение 268 (Тигран 14.11.2026) на живом ядре (настоящая SQLite во временной папке) с настоящим
//! адаптером биржи XT.com, который ходит по HTTP в локальный сервер (моков логики нет; как tests/core_exec_book_238.rs):
//! - справочник пар — живой ответ GET /v4/public/symbol от 06.11.2026 (tests/data/xt_live_2026-11-06/symbol_pepe.json:
//!   btc_usdt, orderTypes [LIMIT, MARKET], timeInForces [GTC, IOC]);
//! - стакан REST — пример ответа GET /v4/public/depth из официальной документации XT
//!   (tests/data/rest_book_2026-11-09/xt_depth_doc.json);
//! - ответы на POST /v4/order — отказы в виде конверта XT {rc, mc, ma} (venues/xt/reply.rs) с кодами официальной
//!   таблицы (https://doc.xt.com/docs/spot/Access%20Description/ResponseCode): на ЛИМИТНЫЙ ордер — ORDER_F0501
//!   («Trigger PROTECTION_LIMIT Filter» → PriceOutOfRange), на РЫНОЧНЫЙ — ORDER_F0601 («Trigger PROTECTION_MARKET
//!   Filter» → обычный отказ). Живых записей этих отказов XT в tests/data нет — тело собрано по описанию конверта.
//!
//! Проверяется путь целиком (sell::sell_or_stuck и кнопка sell::market):
//! - настройка включена: лимитный ордер отклонён «цена вне диапазона» → свежая котировка → продажа выгодна → на биржу
//!   уходит РОВНО ОДИН рыночный ордер (type MARKET, timeInForce IOC, quantity в базовой монете, без price, свой
//!   clientOrderId — не тот, что у лимитного); его отказ — алерт человеку (как раньше), третьего ордера нет: повтора
//!   по кругу нет; строка хронологии «… продаём рыночным ордером (решение 268)»; отметка сделки стоит;
//! - невыгодно — рыночного ордера нет, сделка в режиме застрявшей монеты (здесь stuck.mode не задан — остановлена с
//!   причиной, путь пройден);
//! - кнопка человека — рыночный ордер и при невыгодной продаже;
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

/// Тела запросов POST /v4/order, дошедших до сервера, по порядку.
type Orders = Arc<Mutex<Vec<Value>>>;

fn pw() -> Zeroizing<String> {
    Zeroizing::new("тест".to_string())
}

/// Тело ответа XT: {"rc": 0, "mc": "SUCCESS", "ma": [], "result": …} (README tests/data/xt_live_2026-11-06).
fn symbols_body() -> String {
    let t = std::fs::read_to_string("tests/data/xt_live_2026-11-06/symbol_pepe.json").unwrap();
    let result: Value = serde_json::from_str(&t).unwrap();
    json!({"rc": 0, "mc": "SUCCESS", "ma": [], "result": result}).to_string()
}

/// Запрос целиком: заголовки и тело по content-length. Возвращает: (путь, тело).
async fn request(c: &mut tokio::net::TcpStream) -> (String, String) {
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
    let path = text.split_whitespace().nth(1).unwrap_or("").to_string();
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (path, body)
}

/// Локальный сервер: справочник, стакан и отказы на ордера (см. шапку). Возвращает: адрес и принятые ордера.
async fn serve() -> (String, Orders) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    let orders: Orders = Default::default();
    let depth =
        std::fs::read_to_string("tests/data/rest_book_2026-11-09/xt_depth_doc.json").unwrap();
    let symbols = symbols_body();
    let seen = orders.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut c, _)) = l.accept().await else {
                return;
            };
            let (depth, symbols, seen) = (depth.clone(), symbols.clone(), seen.clone());
            tokio::spawn(async move {
                let (path, body) = request(&mut c).await;
                let (code, body) = if path.starts_with("/v4/public/depth") {
                    ("200 OK", depth)
                } else if path.starts_with("/v4/public/symbol") {
                    ("200 OK", symbols)
                } else if path.starts_with("/v4/order") {
                    let o: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    let mc = if o["type"] == "MARKET" {
                        "ORDER_F0601"
                    } else {
                        "ORDER_F0501"
                    };
                    seen.lock().unwrap().push(o);
                    (
                        "200 OK",
                        json!({"rc": 1, "mc": mc, "ma": [], "result": null}).to_string(),
                    )
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
    (addr, orders)
}

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-268-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Ядро с адаптером XT (ключи заданы, хранилище открыто). on — значение sell.market_on_price_range площадки (None —
/// не задавать). Возвращает: ядро и принятые сервером ордера.
async fn start(d: &Path, on: Option<bool>) -> (Core, Orders) {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let (url, orders) = serve().await;
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
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    (core, orders)
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

/// Сделка: 1 BTC лежит на xt, куплено на bsc_dex; вложено `invested` USDT (0 — любая продажа выгодна).
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

fn types(o: &Orders) -> Vec<String> {
    o.lock()
        .unwrap()
        .iter()
        .map(|x| x["type"].as_str().unwrap_or("?").to_string())
        .collect()
}

#[test]
fn profitable_sale_goes_market_exactly_once() {
    big(async {
        let d = dir("market");
        let (core, orders) = start(&d, Some(true)).await;
        let id = "20261113211300000BTC";
        arb::core::sell::sell_or_stuck(&core, deal(id, "0")).await;
        assert_eq!(
            types(&orders),
            vec!["LIMIT", "MARKET"],
            "ровно один рыночный"
        );
        let sent = orders.lock().unwrap().clone();
        let (limit, market) = (&sent[0], &sent[1]);
        assert!(limit.get("price").is_some());
        assert_eq!(market["side"], "SELL");
        assert_eq!(market["timeInForce"], "IOC");
        assert_eq!(market["bizType"], "SPOT");
        assert_eq!(market["symbol"], limit["symbol"]);
        assert_eq!(
            market["quantity"], limit["quantity"],
            "то же количество базовой монеты"
        );
        for k in ["price", "quoteQty"] {
            assert!(market.get(k).is_none(), "{market}");
        }
        assert_ne!(
            market["clientOrderId"], limit["clientOrderId"],
            "свой ID на попытку"
        );
        let x = saved(&core, id);
        let t = lines(&x);
        assert!(x.price_range_tried);
        assert!(
            t.iter().any(|l| l.starts_with(
                "Биржа отклонила лимитный ордер продажи: цена вне разрешённого диапазона (XT, код ORDER_F0501"
            ) && l.contains("снимок REST")
                && l.contains("продажа выгодна")
                && l.ends_with("продаём рыночным ордером (решение 268)")),
            "{t:#?}"
        );
        // Отказ рыночного ордера — обычный отказ: алерт человеку, монеты на месте, повторов нет.
        assert_eq!(x.step, Step::WaitingHuman, "{t:#?}");
        assert!(
            t.iter()
                .any(|l| l.contains("ORDER_F0601") && l.contains("ждём решения человека")),
            "{t:#?}"
        );
        assert_eq!(x.unsent_retries, 0);
        assert_eq!(x.qty, Decimal::ONE);
    });
}

#[test]
fn unprofitable_sale_goes_to_stuck_mode_without_market_order() {
    big(async {
        let d = dir("stuck");
        let (core, orders) = start(&d, Some(true)).await;
        // Невыгодно: вложено больше выручки любой продажи по стакану примера. Первая продажа запускается «по рынку»
        // (как из режима застрявшей монеты) — sell_or_stuck при «невыгодно» до ордера бы не дошёл.
        let id = "20261113211301000BTC";
        let why = "ждали — выгодно не стало";
        arb::core::sell::market(&core, deal(id, "1000000"), why).await;
        assert_eq!(types(&orders), vec!["LIMIT"], "рыночного ордера нет");
        let x = saved(&core, id);
        let t = lines(&x);
        assert!(x.price_range_tried);
        assert!(
            t.iter()
                .any(|l| l.contains("По стакану продажа здесь невыгодна")
                    && l.ends_with("режим застрявшей монеты (решение 268)")),
            "{t:#?}"
        );
        // stuck.mode в тесте не задан — режим застрявшей монеты останавливает сделку с причиной (путь пройден).
        assert_eq!(x.step, Step::Halted, "{t:#?}");
        assert!(t.iter().any(|l| l.contains("stuck.mode")), "{t:#?}");
    });
}

#[test]
fn button_goes_market_even_if_unprofitable_and_second_refusal_alerts() {
    big(async {
        let d = dir("button");
        let (core, orders) = start(&d, Some(true)).await;
        let id = "20261113211302000BTC";
        let run = arb::core::sell::market(&core, deal(id, "1000000"), arb::core::sell::BUTTON);
        arb::core::risk_count::manual(run).await;
        assert_eq!(types(&orders), vec!["LIMIT", "MARKET"]);
        let x = saved(&core, id);
        let t = lines(&x);
        assert!(
            t.iter()
                .any(|l| l.contains("кнопка человека — без проверки выгодности")
                    && l.ends_with("продаём рыночным ордером (решение 268)")),
            "{t:#?}"
        );
        assert_eq!(x.step, Step::WaitingHuman, "{t:#?}");
        // Та же сделка ещё раз без нового нажатия (отметка стоит): лимитный отказ — сразу алерт, рыночного ордера нет.
        let mut again = x.clone();
        again.step = Step::Selling;
        assert!(again.price_range_tried);
        arb::core::sell::market(&core, again, "порог убытка").await;
        assert_eq!(types(&orders), vec!["LIMIT", "MARKET", "LIMIT"]);
        let x = saved(&core, id);
        assert_eq!(x.step, Step::WaitingHuman);
        let t = lines(&x);
        assert!(
            t.last().unwrap().contains("ORDER_F0501")
                && t.last().unwrap().contains("ждём решения человека"),
            "{t:#?}"
        );
    });
}

#[test]
fn setting_off_keeps_old_behaviour() {
    big(async {
        for (name, on) in [("off", None), ("no", Some(false))] {
            let d = dir(name);
            let (core, orders) = start(&d, on).await;
            let id = "20261113211303000BTC";
            arb::core::sell::sell_or_stuck(&core, deal(id, "0")).await;
            assert_eq!(types(&orders), vec!["LIMIT"], "{name}");
            let x = saved(&core, id);
            let t = lines(&x);
            assert!(!x.price_range_tried, "{name}");
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

/// Кнопка человека при не заданной настройке — как до решения: один лимитный ордер, алерт.
#[test]
fn button_with_setting_off_keeps_old_behaviour() {
    big(async {
        let d = dir("button-off");
        let (core, orders) = start(&d, None).await;
        let id = "20261113211304000BTC";
        let run = arb::core::sell::market(&core, deal(id, "1000000"), arb::core::sell::BUTTON);
        arb::core::risk_count::manual(run).await;
        assert_eq!(types(&orders), vec!["LIMIT"]);
        let x = saved(&core, id);
        assert!(!x.price_range_tried);
        assert_eq!(x.step, Step::WaitingHuman);
        assert!(!lines(&x).iter().any(|l| l.contains("решение 268")));
    });
}
