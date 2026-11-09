//! Файл: tests/window_deal_price.rs
//! Что делает: решение 245 (часть А) — цена DEX на объём нашей сделки в окне (core/window_deal.rs, window_deal_run.rs)
//! на настоящем ядре с настоящими адаптерами BingX и DEX (без сети), настоящей SQLite и записанных данных CAKE
//! (tests/size_common: стакан BingX, пул CAKE/USDT PancakeSwap V2 0xa39a…755b; котировку продажи считает адаптер —
//! quote/pool_quote.rs). Проверяется: сделка bingx → bsc_dex, записанная в учёт, — под наблюдением с отметкой первой
//! точки; пулы маршрута — из плана; window.dex_price* не заданы — котировки нет; заданы — котировка кошельком сделки
//! (кошелька в маппинге нет — причина у сделки и на странице окна, отметка снята); событие чужого пула или пула другой
//! площадки — без отметки, пула маршрута — отметка; интервал не прошёл — отметка ждёт; объём — план, пока монет нет на
//! DEX, и факт, когда они там; точка на объём сделки — в файле окна с пометкой сделки; закрытая сделка снята;
//! DEX → биржа — под наблюдением только до факта покупки.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{window_deal, window_deal_run, window_run, ledger, episode_run}, arb::venues::{bingx,
//! bsc_dex}, tests/size_common.

mod size_common;

use alloy_primitives::address;
use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::types::{
    DexRoute, Direction, PoolRef, Quote, RouteHop, RouteLeg, RouteMode, Side, TradeRequest,
    TradeResult, TradeStatus, VenueId,
};
use arb::core::window::WindowInfo;
use arb::core::window_deal::{self, DealSize};
use arb::core::window_dex::DexPoint;
use arb::core::{episode_run, ledger, window_deal_run, window_run};
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

/// Настоящее ядро с адаптерами BingX и DEX (ключей и адресов нет — сети нет), как tests/window_dex_price.rs.
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

/// Котировка продажи CAKE на DEX по записанному пулу на qty монет с маршрутом «один пул» из записи.
fn sell_quote(rec: &Rec, qty: &str) -> Quote {
    let c = size_common::calc::calc(rec, d(qty));
    let mut q = c.sell_quote;
    q.route = Some(DexRoute {
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
    q
}

/// Факт покупки на площадке (для сделки DEX → биржа): исполнено qty по цене.
fn bought(dir: &Direction, qty: &str) -> TradeResult {
    TradeResult {
        request: TradeRequest {
            venue: dir.buy_on.clone(),
            asset: dir.asset.clone(),
            side: Side::Buy,
            qty: d(qty),
            worst_price: d("3"),
            deal_id: "D2".into(),
            route: None,
            exit_position: false,
        },
        status: TradeStatus::Filled,
        filled_qty: d(qty),
        avg_price: d("2.5"),
        fees: vec![],
        venue_ref: "0x01".into(),
        chain: None,
        finished_at: chrono::Utc::now(),
        executor: None,
    }
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
async fn window_writes_dex_price_on_deal_size() {
    let dir = std::env::temp_dir().join(format!("arb-window-deal-{}", std::process::id()));
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
    let bxs = Scope::Venue("bingx".into());
    for (n, v) in [
        ("venue.quote_asset", json!("USDT")),
        ("venue.depth_levels", json!(20)),
    ] {
        s.set(n, bxs.clone(), Some(v), "тест").await.unwrap();
    }
    let rec = Rec::load("window-deal").await;
    let mut calc = size_common::calc::calc(&rec, d("10"));
    calc.sell_quote = sell_quote(&rec, "10");
    let dir_ = calc.direction.clone();
    window_run::pass(&core, &calc, true);
    for _ in 0..50 {
        if bx.taps.has("CAKE") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(bx.taps.has("CAKE"), "окно открыто");

    // Сделка в учёте — под наблюдением с отметкой первой точки; настройки цены DEX не заданы — котировки нет.
    let plan = serde_json::to_value(&calc).unwrap();
    let mut deal = Deal::new("D1".into(), dir_.clone(), plan, calc.profit_usd);
    ledger::save(&core, &deal).await;
    assert_eq!(window_deal::pending(&core), ["D1"]);
    assert_eq!(window_deal_run::step(&core).await, None);
    assert_eq!(
        window_deal::pending(&core),
        ["D1"],
        "без настроек отметка ждёт"
    );

    // Заданы — котировка кошельком сделки: кошелька DEX в маппинге нет — причина у сделки, отметка снята.
    set("window.dex_price", json!(true)).await;
    set("window.dex_price_min_ms", json!(3_600_000)).await;
    window_deal_run::step(&core).await;
    assert!(window_deal::pending(&core).is_empty());
    let errs = window_deal::errors(&core, &dir_.asset);
    assert!(errs["D1"].contains("кошел"), "{errs:?}");

    // Чужой пул и пул другой площадки — без отметки; пул маршрута плана — отметка; интервал не прошёл — ждёт.
    let dex = VenueId("bsc_dex".into());
    let other = PoolRef::Contract(address!("0x0000000000000000000000000000000000000001"));
    window_deal::touched(&core, &dex, &other);
    window_deal::touched(&core, &VenueId("bingx".into()), &PoolRef::Contract(PAIR));
    assert!(window_deal::pending(&core).is_empty());
    window_deal::touched(&core, &dex, &PoolRef::Contract(PAIR));
    assert_eq!(window_deal::pending(&core), ["D1"]);
    assert!(
        window_deal_run::step(&core).await.is_some(),
        "проснуться к концу интервала"
    );
    assert_eq!(
        window_deal::pending(&core),
        ["D1"],
        "интервал не прошёл — отметка ждёт"
    );

    // Объём: монет на DEX нет — план (sell_qty), монеты на DEX — факт.
    assert_eq!(
        window_deal_run::qty_of(&deal, Side::Sell, &dex),
        (Some(calc.sell_qty), "plan")
    );
    deal.at = dex.clone();
    deal.qty = d("9.5");
    deal.step = Step::Selling;
    ledger::save(&core, &deal).await;
    assert_eq!(
        window_deal_run::qty_of(&deal, Side::Sell, &dex),
        (Some(d("9.5")), "fact")
    );

    // Точка на объём сделки — в окно направления (котировка — адаптером по записанному пулу).
    let q = sell_quote(&rec, "9.5");
    let size = DealSize {
        id: "D1".into(),
        qty_from: "fact".into(),
        asked: d("9.5"),
    };
    assert!(window_deal_run::write(
        &core,
        &dir_,
        Side::Sell,
        &q,
        size.clone()
    ));

    // Закрытая сделка снята с наблюдения (и её причина).
    deal.step = Step::Closed;
    ledger::save(&core, &deal).await;
    assert!(window_deal::pending(&core).is_empty());
    assert!(window_deal::errors(&core, &dir_.asset).is_empty());

    // DEX → биржа: под наблюдением только до факта покупки на DEX.
    let back = Direction {
        asset: dir_.asset.clone(),
        buy_on: dex.clone(),
        sell_on: VenueId("bingx".into()),
    };
    let mut d2 = Deal::new("D2".into(), back.clone(), json!({"qty": "10"}), d("0"));
    ledger::save(&core, &d2).await;
    assert_eq!(window_deal::pending(&core), ["D2"]);
    assert_eq!(
        window_deal_run::qty_of(&d2, Side::Buy, &dex),
        (Some(d("10")), "plan")
    );
    d2.buy = Some(bought(&back, "10"));
    d2.step = Step::Transfer;
    ledger::save(&core, &d2).await;
    assert!(
        window_deal::pending(&core).is_empty(),
        "после покупки — не наблюдается"
    );
    // Сделка монеты открыта — окно ждёт её конца (решение 125): закрываем, чтобы окно закрылось.
    d2.step = Step::Closed;
    ledger::save(&core, &d2).await;

    episode_run::failed(&core, &dir_, "прибыль ниже порога");
    let info = closed(&db).await;
    assert_eq!(info.dex_points, 1);
    let mut page = Value::Null;
    for _ in 0..50 {
        page = core.window(&info.id, 0, 100, 10).await.unwrap().unwrap();
        if page["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["type"] == "close")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let pts: Vec<DexPoint> = serde_json::from_value(page["dex"].clone()).unwrap();
    assert_eq!(pts.len(), 1);
    let p = &pts[0];
    assert_eq!(p.deal.as_ref(), Some(&size));
    assert_eq!(
        (p.price, p.qty, p.side),
        (q.avg_price, q.fillable_qty, Side::Sell)
    );
    assert_eq!(p.pools, [format!("{PAIR:#x}")]);
    assert_eq!(p.block, Some(rec.pool.pool.block));
    let _ = std::fs::remove_dir_all(&dir);
}
