//! Файл: tests/t175_common/mod.rs
//! Что делает: стенд теста tests/core_sell_stuck_chain_t175.rs (решение 389) — живое ядро на настоящей SQLite во
//! временной папке и настоящий адаптер XT.com, который ходит по HTTP в локальный сервер (как
//! tests/core_market_sell_268.rs). Сервер отдаёт: справочник пар — живой ответ XT 06.11.2026
//! (tests/data/xt_live_2026-11-06/symbol_pepe.json); стакан — пример документации
//! (tests/data/rest_book_2026-11-09/xt_depth_doc.json); POST /v4/order — {orderId} (Submit order); GET /v4/order —
//! состояние по полям Query single: state CANCELED, executedQty 0 (ордер IOC снят без исполнения; собрано по
//! документации, не записано); GET /v4/balances — 1 BTC на споте (решение 413: перед продажей бот читает баланс;
//! согласовано со сделкой стенда — 1 BTC, ордера не исполняются; собрано по документации Get balances); прочее — 404.
//! Кто вызывает: tests/core_sell_stuck_chain_t175.rs.
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

/// Сделка теста.
pub const ID: &str = "20261117143812842BTC";

/// Сколько ордеров POST /v4/order дошло до сервера.
pub type Srv = Arc<Mutex<usize>>;

/// Стек потока теста — тот же, что у tests/core_market_sell_268.rs (там же замер: один уровень продажи с отказом
/// ордера в отладочной сборке ≈ 1,3 МиБ у автоматики). Параметр теста, не настройка бота.
const STACK: usize = 16 * 1024 * 1024;

/// Выполняет тело теста в потоке со стеком STACK на своём однопоточном рантайме. Паника тела — паника теста.
pub fn big<F>(body: F)
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

/// Тело ответа XT {"rc": 0, "mc": "SUCCESS", "ma": [], "result": …}.
fn ok(result: Value) -> String {
    json!({"rc": 0, "mc": "SUCCESS", "ma": [], "result": result}).to_string()
}

/// Запрос целиком: заголовки и тело по content-length. Возвращает: (метод, путь).
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
    let mut w = text.split_whitespace();
    let method = w.next().unwrap_or("").to_string();
    let path = w.next().unwrap_or("").to_string();
    (method, path)
}

/// Ответ сервера на запрос (см. шапку). Принимает: метод, путь, счётчик ордеров. Возвращает: (статус, тело).
fn answer(method: &str, path: &str, n: &Srv) -> (&'static str, String) {
    if path.starts_with("/v4/public/depth") {
        let t =
            std::fs::read_to_string("tests/data/rest_book_2026-11-09/xt_depth_doc.json").unwrap();
        return ("200 OK", t);
    }
    if path.starts_with("/v4/public/symbol") {
        let t = std::fs::read_to_string("tests/data/xt_live_2026-11-06/symbol_pepe.json").unwrap();
        return ("200 OK", ok(serde_json::from_str(&t).unwrap()));
    }
    // Решение 413 (повторное ревью t198, Б-3: баланс биржи не прочитан — продажи нет): баланс спота GET /v4/balances
    // (https://doc.xt.com/docs/spot/Balance/GetBalances: result.assets[] — currency, availableAmount, frozenAmount),
    // согласованный со стендом: у сделки 1 BTC (deal), ни один ордер не исполняется (CANCELED, executedQty 0) — на
    // счёте всё время 1 BTC.
    if path.starts_with("/v4/balances") {
        let a =
            json!({"assets": [{"currency": "btc", "availableAmount": "1", "frozenAmount": "0"}]});
        return ("200 OK", ok(a));
    }
    if path.starts_with("/v4/order") && method == "POST" {
        let mut c = n.lock().unwrap();
        *c += 1;
        return ("200 OK", ok(json!({"orderId": format!("L{c}")})));
    }
    if path.starts_with("/v4/order") && method == "GET" {
        let s = json!({"symbol": "btc_usdt", "orderId": "L", "state": "CANCELED", "type": "LIMIT",
                       "side": "SELL", "origQty": "1", "executedQty": "0", "leavingQty": "1",
                       "avgPrice": "0", "tradeQuote": "0", "fee": "0", "feeCurrency": "usdt"});
        return ("200 OK", ok(s));
    }
    ("404 Not Found", "{}".to_string())
}

/// Локальный сервер. Возвращает: адрес и счётчик ордеров.
async fn serve() -> (String, Srv) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    let srv: Srv = Default::default();
    let s = srv.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut c, _)) = l.accept().await else {
                return;
            };
            let s = s.clone();
            tokio::spawn(async move {
                let (method, path) = request(&mut c).await;
                let (code, body) = answer(&method, &path, &s);
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
pub fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Ядро с адаптером XT (ключи заданы, хранилище открыто). Возвращает: ядро, настройки, журнал, счётчик ордеров.
pub async fn start(d: &Path) -> (Core, Store, Journal, Srv) {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let (url, srv) = serve().await;
    let vault = arb::secrets::Vault::locked(d.join("secrets.enc"));
    let pw = || Zeroizing::new("тест".to_string());
    vault.create(pw(), KDF).await.unwrap();
    for n in ["xt.api_key", "xt.api_secret"] {
        let v = Some(Zeroizing::new("k".to_string()));
        vault.change(pw(), n, v, KDF).await.unwrap();
    }
    let g = |n: &str, v: Value| (n.to_string(), Scope::Global, v);
    let x = |n: &str, v: Value| (n.to_string(), Scope::Venue("xt".into()), v);
    let list = vec![
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

/// Задаёт глобальную настройку. Ошибок нет: паника теста.
pub async fn set(s: &Store, name: &str, v: Value) {
    s.set(name, Scope::Global, Some(v), "тест").await.unwrap();
}

/// Сделка: 1 BTC лежит на xt, куплено на bsc_dex; вложено `invested` USDT. По стакану примера 1 BTC продаётся
/// примерно за 199 USDT: при 200 вложенных продавать невыгодно, убыток ≈ 0,4 %.
pub fn deal(id: &str, invested: &str) -> Deal {
    let dir = Direction {
        asset: Asset {
            chain: None,
            ticker: "BTC".into(),
            contract: None,
        },
        buy_on: VenueId("bsc_dex".into()),
        sell_on: VenueId("xt".into()),
    };
    let mut deal = Deal::new(id.into(), dir, json!(null), Decimal::ZERO);
    deal.at = VenueId("xt".into());
    deal.qty = Decimal::ONE;
    deal.invested = Decimal::from_str(invested).unwrap();
    deal.step = Step::Selling;
    deal
}

/// Сохранённая сделка. Ошибок нет: паника теста.
pub fn saved(core: &Core, id: &str) -> Deal {
    core.deals().into_iter().find(|d| d.id == id).unwrap()
}

/// Строки хронологии сделки.
pub fn lines(d: &Deal) -> Vec<String> {
    d.timeline.iter().map(|(_, t)| t.clone()).collect()
}

/// Сколько ордеров дошло до сервера.
pub fn orders(s: &Srv) -> usize {
    *s.lock().unwrap()
}

/// Виды записей журнала из списка `keep` в порядке записи (старые первыми). Ошибок нет: паника теста.
pub async fn kinds(journal: &Journal, keep: &[&str]) -> Vec<String> {
    let mut all = journal.list(1_000_000, None).await.unwrap();
    all.reverse();
    all.into_iter()
        .filter(|e| keep.contains(&e.kind.as_str()))
        .map(|e| e.kind)
        .collect()
}
