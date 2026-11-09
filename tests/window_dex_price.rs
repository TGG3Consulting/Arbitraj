//! Файл: tests/window_dex_price.rs
//! Что делает: решение 241 — цена пула DEX в окне (core/window_dex.rs) на настоящем ядре с настоящими адаптерами BingX
//! и DEX (без сети), настоящей SQLite и записанных данных CAKE (tests/size_common: стакан BingX, пул CAKE/USDT
//! PancakeSwap V2 0xa39a…755b на блоке из записи; котировку продажи на DEX считает адаптер — quote/pool_quote.rs).
//! Проверяется: окно bingx → bsc_dex открыто; window.dex_price* не заданы — точки нет, причина на странице окна; заданы —
//! первая точка сразу; та же поколение пулов при другой цене — точки нет; пулы монеты изменились — вторая точка;
//! window.dex_price_min_ms не прошёл — точки нет; закрытое окно — в сводке точек 2, на странице ряд `dex` с ценой,
//! объёмом, блоком и пулом маршрута котировки, служебные строки файла без «dex».
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{window_run, window_dex, episode_run}, arb::venues::{bingx, bsc_dex}, tests/size_common.

mod size_common;

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::types::{DexRoute, Quote, RouteHop, RouteLeg, RouteMode, Side};
use arb::core::window::WindowInfo;
use arb::core::{episode_run, window_dex, window_run};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::windows_db::{self, Filter};
use serde_json::{Value, json};
use size_common::{PAIR, Rec, d};
use tokio::sync::broadcast;

/// Настоящее ядро с адаптерами BingX и DEX (ключей и адресов нет — сети нет): DEX нужен ядру как вид площадки
/// (VenueKind::Dex) — сторона DEX направления. Возвращает ядро, хранилище, базу и окружение BingX (приёмники окон).
async fn start(d: &std::path::Path) -> (Core, Store, Db, arb::venues::bingx::ctx::Ctx) {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let vault = arb::secrets::Vault::locked(d.join("secrets.enc"));
    let raw = arb::storage::raw_log::RawLog::start(d.to_path_buf(), settings.clone());
    let bx = arb::venues::bingx::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: reqwest::Client::new(),
        journal: journal.clone(),
        raw: raw.clone(),
        taps: Default::default(),
    };
    let dx = arb::venues::bsc_dex::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: reqwest::Client::new(),
        journal: journal.clone(),
        raw,
        pace: Default::default(),
        failover: Default::default(),
        account: None,
        index: Default::default(),
    };
    let gw = Registry::new();
    gw.register(std::sync::Arc::new(arb::venues::bingx::Bingx::new(
        bx.clone(),
    )))
    .await;
    gw.register(std::sync::Arc::new(arb::venues::bsc_dex::BscDex::new(dx)))
        .await;
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault,
        journal: journal.clone(),
    };
    let deps = Deps {
        gw,
        settings: settings.clone(),
        journal,
        notifier,
        db: db.clone(),
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    };
    (Core::start(deps).await, settings, db, bx)
}

/// Котировка продажи CAKE на DEX по записанному пулу на qty монет, с маршрутом «один пул» из записи (пул, его вид,
/// блок состояния). gas_units — 0: газа в записях нет, и точке окна он не нужен.
fn dex_quote(rec: &Rec, qty: &str) -> (Quote, Quote) {
    let c = size_common::calc::calc(rec, d(qty));
    let mut sell = c.sell_quote.clone();
    sell.route = Some(DexRoute {
        mode: RouteMode::Direct,
        legs: vec![RouteLeg {
            share: d("1"),
            hops: vec![RouteHop {
                family: rec.pool.family.id.to_string(),
                kind: rec.pool.pool.kind,
                pool: rec.pool.pool.reference.clone(),
            }],
            expected_out: c.proceeds,
        }],
        block: rec.pool.pool.block,
        gas_units: 0,
        executable: true,
        exec_note: None,
    });
    (c.buy_quote, sell)
}

/// Ждёт (не дольше 5 с) закрытую сводку окна в базе.
async fn closed(db: &Db) -> WindowInfo {
    for _ in 0..50 {
        let (rows, _) = windows_db::page(db, Filter::default(), 10, 0)
            .await
            .unwrap();
        if let Some(w) = rows
            .iter()
            .map(|r| serde_json::from_str::<WindowInfo>(r).unwrap())
            .find(|w| w.ended_at.is_some())
        {
            return w;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("окно не закрылось за 5 с");
}

#[tokio::test]
async fn window_writes_dex_price_points() {
    let dir = std::env::temp_dir().join(format!("arb-window-dex-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (core, s, db, bx) = start(&dir).await;
    let set = |n: &'static str, v: Value| {
        let s = s.clone();
        async move { s.set(n, Scope::Global, Some(v), "тест").await.unwrap() }
    };
    for (n, v) in [
        ("window.min_hold_ms", 0),
        ("window.queue_max", 100),
        ("window.max_mb", 10),
        ("window.keep_days", 7),
        ("window.zstd_level", 3),
        ("window.cleanup_s", 3600),
    ] {
        set(n, json!(v)).await;
    }
    set("window.enabled", json!(true)).await;
    // Запись стакана BingX в окне — канал CAKE-USDT@depth20 (как в tests/core_windows.rs).
    let bxs = Scope::Venue("bingx".into());
    for (n, v) in [
        ("venue.quote_asset", json!("USDT")),
        ("venue.depth_levels", json!(20)),
    ] {
        s.set(n, bxs.clone(), Some(v), "тест").await.unwrap();
    }
    let rec = Rec::load("window-dex").await;
    let calc = size_common::calc::calc(&rec, d("10"));
    let dir_ = calc.direction.clone();
    window_run::pass(&core, &calc, true);
    for _ in 0..50 {
        if bx.taps.has("CAKE") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(bx.taps.has("CAKE"), "окно открыто, запись BingX включена");
    // Окно открыто, DEX — вид площадки шлюза, у монеты адрес: поколение пулов есть (событий пулов не было — 0).
    assert_eq!(window_dex::generation(&core, &dir_), Some(0));

    let (qa, q10) = dex_quote(&rec, "10");
    let (_, q20) = dex_quote(&rec, "20");
    assert_ne!(
        q10.avg_price, q20.avg_price,
        "на другом объёме цена пула другая"
    );
    // Настройки не заданы — точки нет.
    window_dex::note(&core, &dir_, &qa, &q10, Some(1));
    set("window.dex_price", json!(true)).await;
    set("window.dex_price_min_ms", json!(0)).await;
    window_dex::note(&core, &dir_, &qa, &q10, Some(1)); // первая — сразу
    window_dex::note(&core, &dir_, &qa, &q20, Some(1)); // пулы не менялись — нет
    window_dex::note(&core, &dir_, &qa, &q20, Some(2)); // пулы изменились — вторая
    set("window.dex_price_min_ms", json!(3_600_000)).await;
    window_dex::note(&core, &dir_, &qa, &q10, Some(3)); // интервал не прошёл — нет

    episode_run::failed(&core, &dir_, "прибыль ниже порога");
    let info = closed(&db).await;
    assert_eq!(info.dex_points, 2);
    let mut page = Value::Null;
    for _ in 0..50 {
        page = core.window(&info.id, 0, 100, 10).await.unwrap().unwrap();
        let lines = page["lines"].as_array().unwrap();
        if lines.iter().any(|l| l["type"] == "close") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let kinds: Vec<&str> = page["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["open", "tap", "close"],
        "точки DEX — не в служебных строках"
    );
    assert!(page["dex_off"].is_null(), "{}", page["dex_off"]);
    let pts: Vec<window_dex::DexPoint> = serde_json::from_value(page["dex"].clone()).unwrap();
    assert_eq!(pts.len(), 2);
    for (p, q) in pts.iter().zip([&q10, &q20]) {
        assert_eq!((p.price, p.qty), (q.avg_price, q.fillable_qty));
        assert_eq!(p.side, Side::Sell);
        assert_eq!(p.venue.0, "bsc_dex");
        assert_eq!(
            (p.buy_on.0.as_str(), p.sell_on.0.as_str()),
            ("bingx", "bsc_dex")
        );
        assert_eq!(p.block, Some(rec.pool.pool.block));
        assert_eq!(p.pools, [format!("{PAIR:#x}")]);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn dex_price_off_reason_shown() {
    let dir = std::env::temp_dir().join(format!("arb-window-dex-off-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (core, s, _db, _bx) = start(&dir).await;
    let off = window_dex::cfg(&s).unwrap_err();
    assert!(off.contains("window.dex_price"), "{off}");
    s.set(
        "window.dex_price",
        Scope::Global,
        Some(json!(false)),
        "тест",
    )
    .await
    .unwrap();
    assert_eq!(window_dex::cfg(&s).unwrap_err(), "window.dex_price = нет");
    s.set("window.dex_price", Scope::Global, Some(json!(true)), "тест")
        .await
        .unwrap();
    let off = window_dex::cfg(&s).unwrap_err();
    assert!(off.contains("window.dex_price_min_ms"), "{off}");
    // Окна нет — поколения нет (тик не платит за котировки без окна).
    assert_eq!(
        window_dex::generation(&core, &size_common::calc::direction()),
        None
    );
    let _ = std::fs::remove_dir_all(&dir);
}
