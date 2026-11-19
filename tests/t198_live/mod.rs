//! Файл: tests/t198_live/mod.rs
//! Что делает: решение 413 (t198, после шестого ревью) — стенд с ЧИТАЕМЫМ балансом биржи для тестов решения остатка
//! под замком: живое ядро на SQLite во временной папке и настоящий адаптер XT.com, который ходит по HTTP в локальный
//! сервер (как tests/t175_common, tests/core_exec_book_238.rs: хранилище открыто тестовым паролем, ключи XT, venue.
//! recv_window_ms). Сервер: справочник пар — живой ответ XT 06.11.2026 (tests/data/xt_live_2026-11-06/symbol_pepe.json);
//! стакан — пример документации (tests/data/rest_book_2026-11-09/xt_depth_doc.json); GET /v4/balances — баланс BTC на
//! споте из ячейки стенда (документация Get balances: result.assets[] — currency, availableAmount, frozenAmount),
//! ответ можно задержать (окно «человек нажал кнопку во время чтения»); GET /v4/balance?currency= — та же ячейка (одна
//! монета, документация Get single currency asset); POST /v4/order — {orderId}; GET /v4/order —
//! CANCELED, executedQty 0 (ордер IOC снят без исполнения; по документации Query single); прочее — 404.
//! Это сервер-стенд по формату официальной документации XT; справочник пар — живая запись (в ней есть пара btc_usdt
//! — монета тестов BTC); значения баланса и ордеров задаёт тест (как tests/t175_common, tests/core_exec_book_238.rs).
//! Сервер — serve.rs.
//! Кто вызывает: tests/core_credit_rest_live_t198.rs.
//! От чего зависит: arb::core, arb::venues::xt, arb::settings, arb::storage, arb::gateway, arb::secrets.

#![allow(dead_code)]

mod serve;

use std::path::Path;
use std::sync::{Arc, Mutex};

use arb::core::Core;
use arb::core::deps::Deps;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::secrets::crypto::KdfParams;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use serde_json::{Value, json};
use tokio::sync::broadcast;
use zeroize::Zeroizing;

const KDF: KdfParams = KdfParams {
    memory_kib: 64,
    iterations: 1,
    parallelism: 1,
};

/// Стек потока теста (путь продажи кнопкой глубокий — как tests/t175_common). Параметр теста, не настройка бота.
const STACK: usize = 16 * 1024 * 1024;

/// Ячейка стенда: баланс BTC на споте и задержка ответа баланса (мс).
#[derive(Clone, Default)]
pub struct Stand {
    pub btc: Arc<Mutex<String>>,
    pub delay_ms: Arc<Mutex<u64>>,
}

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
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap();
}

/// Ядро с адаптером XT (ключи заданы, хранилище открыто) и ячейка стенда; баланс BTC — `btc`.
pub async fn start(d: &Path, btc: &str) -> (Core, Stand) {
    let _ = std::fs::remove_dir_all(d);
    std::fs::create_dir_all(d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let stand = Stand::default();
    *stand.btc.lock().unwrap() = btc.to_string();
    let url = serve::serve(stand.clone()).await;
    let vault = arb::secrets::Vault::locked(d.join("secrets.enc"));
    let pw = || Zeroizing::new("тест".to_string());
    vault.create(pw(), KDF).await.unwrap();
    for n in ["xt.api_key", "xt.api_secret"] {
        let v = Some(Zeroizing::new("k".to_string()));
        vault.change(pw(), n, v, KDF).await.unwrap();
    }
    let g = |n: &str, v: Value| (n.to_string(), Scope::Global, v);
    let x = |n: &str, v: Value| (n.to_string(), Scope::Venue("xt".into()), v);
    // Как tests/t175_common (порядок и значения).
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
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    (core, stand)
}
