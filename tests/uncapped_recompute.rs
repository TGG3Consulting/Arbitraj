//! Файл: tests/uncapped_recompute.rs
//! Что проверяет: решение 218 — пересчёт старых записей «без потолка» по окну сделки. Данные — записанные кадры стакана
//! BingX CAKE-USDT@depth20 (tests/data/bingx/ws_spot_2026-10-23.jsonl), правила пары и сеть BEP20 CAKE у BingX
//! (tests/size_common). Ставки taker аккаунта в записях нет — 0,1 % (план комиссии один у обоих путей).
//! 1) Стакан на момент решения из окна (core/window_at.rs) — кадр строго до момента, а не после (наша покупка).
//! 2) Пересчёт по окну (источник uncapped_replay.rs, план — расчёт той же модели) даёт тот же итог, что расчёт по
//!    снимку того же стакана (решение 217: costs_source::Live со снимком core/frozen.rs) — одна модель (costs.rs
//!    evaluate_with) и один поиск (size_uncapped.rs).
//! 3) Правила полного стакана (решение 221): снимок торгового потока (по нему бот котировал) — раньше полного по
//!    изменениям (BingX @incrDepth «all»); полный по изменениям — запасной, когда торгового снимка нет или он сброшен;
//!    только изменения без полного — нет стакана; снимок, после которого шли неприменимые изменения, без полного по
//!    изменениям — нет стакана.
//! 4) Сквозной пересчёт (uncapped_recompute.rs one) на SQLite: биржа ↔ биржа и биржа → DEX пересчитываются (basis,
//!    момент решения, начало как у старой записи), DEX → биржа и сделка без окна — нет (причина); новая запись —
//!    основная для сделки (by_deal, latest_per_deal); внешняя запись при работающем боте — отказ (open_existing).
//! 5) Пометка источника: Telegram (deal_ceiling.rs — «♻️ … пересчитано по окну»), подпись отчёта Excel (basis_label).
//!
//! Вторая биржа в тестах «биржа ↔ биржа» — id mexc с кадрами BingX: разбор кадров передаётся пересчёту функцией
//! (контракт Venue::decode_tap), тест отдаёт для обоих id разбор BingX — записанных стаканов MEXC в репозитории нет.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{uncapped_recompute*, uncapped_replay, window_at, costs, size_uncapped}, tests/size_common.

mod size_common;

use std::sync::Arc;

use arb::core::Core;
use arb::core::costs::{self, Live};
use arb::core::deal::Deal;
use arb::core::deps::Deps;
use arb::core::dirctx::DirCtx;
use arb::core::frozen::Frozen;
use arb::core::registry::Presence;
use arb::core::size::Params;
use arb::core::size_curve::{SourceCurve, cumulative};
use arb::core::size_uncapped::uncapped;
use arb::core::types::{Asset, Direction, OrderBook, Side, TapDecoded, VenueId};
use arb::core::uncapped_recompute::{Env, G_CEX, G_DEX, one};
use arb::core::uncapped_replay::{Plan, Replay};
use arb::core::uncapped_sum::latest_per_deal;
use arb::core::uncapped_types::{BASIS_RECOMPUTED, BASIS_RECOMPUTED_DEX_PLAN, Brief};
use arb::core::window::{WinDeal, WinVenue, WindowInfo};
use arb::core::window_at::books_before;
use arb::gateway::frozen::{BookFee, FrozenBook};
use arb::gateway::registry::Registry;
use arb::gateway::tap::{TapData, TapStream};
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::uncapped_db::{self, DealRow};
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use size_common::{CAKE, Rec, USDT, d};
use tokio::sync::broadcast;

/// Разбор кадров окна: оба id — разбор BingX (см. шапку).
fn decode(_v: &VenueId, s: TapStream, data: &TapData) -> Result<TapDecoded, String> {
    arb::venues::bingx::tap::decode(s, data).map_err(|e| e.message)
}

fn cake() -> Asset {
    Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    }
}

fn dir(b: &str) -> Direction {
    Direction {
        asset: cake(),
        buy_on: VenueId("bingx".into()),
        sell_on: VenueId(b.into()),
    }
}

/// Записанный кадр CAKE-USDT@depth20 («до покупки») и он же без лучшего продавца — «после нашей покупки» (как MCRT
/// 07.11: покупка съела весь уровень; второго кадра CAKE в записи нет).
fn frames() -> (String, String) {
    let text = std::fs::read_to_string("tests/data/bingx/ws_spot_2026-10-23.jsonl").unwrap();
    let before = text
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["m"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .find(|m| m.contains("CAKE-USDT@depth") && m.contains("\"data\""))
        .unwrap();
    let mut v: Value = serde_json::from_str(&before).unwrap();
    let asks = v["data"]["asks"].as_array_mut().unwrap();
    let price = |l: &Value| d(l[0].as_str().unwrap());
    let best = (0..asks.len()).min_by_key(|&i| price(&asks[i])).unwrap();
    asks.remove(best);
    (before, v.to_string())
}

/// Строка файла окна (как core/window_tap.rs line).
fn line(v: &str, s: &str, ns: i64, t: &str) -> String {
    json!({"type": "f", "v": v, "s": s, "c": "t", "ns": ns, "t": t}).to_string()
}

fn ns(t: DateTime<Utc>) -> i64 {
    t.timestamp_nanos_opt().unwrap()
}

/// Контекст направления на записанных данных (правила пары и сеть BingX).
fn cx(rec: &Rec, b: &str) -> DirCtx {
    let p = |v: &str| Presence {
        venue: VenueId(v.into()),
        ticker: "CAKE".into(),
        network: Some("BEP20".into()),
        can_receive: true,
        can_send: true,
        can_buy: true,
        can_sell: true,
        excluded: vec![],
        info: Some(rec.net.clone()),
        via: None,
    };
    DirCtx {
        dir: dir(b),
        a: p("bingx"),
        b: p(b),
        net_a: "BEP20".into(),
        net_b: "BEP20".into(),
        quote: vec!["USDT".into(), "USDT".into()],
        usdt: Asset {
            ticker: "USDT".into(),
            bsc_contract: Some(USDT),
        },
        cex: vec![],
        sigma: d("0.0005"),
        transit_s: d("120"),
        k: d("1"),
        h: d("1"),
        closure_share: None,
        return_per_trade: false,
        pair_a: Some(rec.pair.clone()),
        pair_b: Some(rec.pair.clone()),
        usdt_b: None,
        transfer_tax: Some(Decimal::ZERO),
    }
}

/// Снимок стороны (как адаптер BingX в момент решения): покупка — комиссия в монете, продажа — от суммы в USDT.
fn frozen_side(b: &OrderBook, side: Side) -> FrozenBook {
    let (per_coin, asset) = match side {
        Side::Buy => (true, "CAKE"),
        Side::Sell => (false, "USDT"),
    };
    FrozenBook {
        venue: b.venue.clone(),
        asset: cake(),
        side,
        book: b.clone(),
        fee: BookFee {
            rate: d("0.001"),
            per_coin,
            asset: asset.into(),
            paid_in: None,
        },
        taken_at: b.received_at,
    }
}

/// Настоящее ядро без площадок (снимок и справочник сетей — без сети).
async fn core(dir: &std::path::Path) -> (Core, Store, Db) {
    let db = Db::open(&dir.join("arb.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let vault = arb::secrets::Vault::locked(dir.join("secrets.enc"));
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault,
        journal: journal.clone(),
    };
    let deps = Deps {
        gw: Registry::new(),
        settings: settings.clone(),
        journal,
        notifier,
        db: db.clone(),
        changes,
        buffer: 64,
        data_dir: dir.to_path_buf(),
    };
    (Core::start(deps).await, settings, db)
}

fn tmp(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("arb_recompute_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(p.join("windows")).unwrap();
    p
}

/// Параметры поиска (общие для обоих путей).
fn params(rec: &Rec, a: &OrderBook, b: &OrderBook) -> Params {
    Params {
        step: rec.pair.step_size,
        budget: d("100000"),
        guess: d("20") / a.asks[0].price,
        buy_breaks: cumulative(&a.asks),
        sell_breaks: cumulative(&b.bids),
        bisect_steps: 30,
        grid_points: 20,
        refine_steps: 30,
        tolerance_pct: d("0.1"),
        max_evals: 400,
    }
}

#[tokio::test]
async fn window_recompute_equals_snapshot() {
    let dirp = tmp("eq");
    let (core, _s, _db) = core(&dirp).await;
    let rec = Rec::load("recompute-eq").await;
    let (before, after) = frames();
    let t0 = Utc::now();
    let t = t0 + Duration::milliseconds(5);
    let rows = vec![
        line("bingx", "Book", ns(t0), &before),
        line("mexc", "Book", ns(t0) + 1, &before),
        line("bingx", "Book", ns(t) + 1, &after),
        line("mexc", "Book", ns(t) + 2, &after),
    ];
    let want = [VenueId("bingx".into()), VenueId("mexc".into())];
    let mut books = books_before(&rows, &want, &cake(), ns(t), &decode);
    let a = books.remove(&want[0]).unwrap().unwrap();
    let b = books.remove(&want[1]).unwrap().unwrap();
    assert!(!a.full_depth);
    let mut later = books_before(&rows, &want, &cake(), ns(t) + 10, &decode);
    let a_after = later.remove(&want[0]).unwrap().unwrap();
    assert_ne!(
        a.book.asks, a_after.book.asks,
        "кадр после момента решения не берётся"
    );
    // Снимок того же стакана (решение 217) — источник Live со снимком.
    let c = cx(&rec, "mexc");
    let fz = Frozen {
        a: Arc::new(frozen_side(&a.book, Side::Buy)),
        b: Arc::new(frozen_side(&b.book, Side::Sell)),
        at: t,
        decided: None,
    };
    let live = Live {
        core: &core,
        src: Some(&fz),
    };
    // План сделки — расчёт той же модели на плановом объёме (как executor.rs: Calc в JSON).
    let plan_calc = costs::evaluate_with(&c, d("30"), &live).await.unwrap();
    let plan = Plan::parse(&serde_json::to_value(&plan_calc).unwrap()).unwrap();
    let replay = Replay::new(
        &c.dir,
        &plan,
        a.book.clone(),
        Some(b.book.clone()),
        ["USDT", "USDT"],
    )
    .unwrap();
    let p = params(&rec, &a.book, &b.book);
    let ladder = [d("50"), d("200"), d("1000")];
    let pass = |pt: &arb::core::size::Point<costs::Calc>| pt.profit > Decimal::ZERO;
    let mut on_snap = SourceCurve { cx: &c, src: &live };
    let s = uncapped(&mut on_snap, &p, d("20"), &ladder, pass, None)
        .await
        .unwrap();
    let mut on_window = SourceCurve {
        cx: &c,
        src: &replay,
    };
    let w = uncapped(&mut on_window, &p, d("20"), &ladder, pass, None)
        .await
        .unwrap();
    let close = |x: Decimal, y: Decimal| (x - y).abs() < d("0.000000001");
    assert_eq!(s.opt.qty, w.opt.qty);
    assert!(
        close(s.opt.profit, w.opt.profit),
        "{} vs {}",
        s.opt.profit,
        w.opt.profit
    );
    assert!(close(s.opt.invested, w.opt.invested));
    assert_eq!(s.upper_qty, w.upper_qty);
    assert_eq!(s.min_plus.map(|m| m.qty), w.min_plus.map(|m| m.qty));
    for (x, y) in s.levels.iter().zip(&w.levels) {
        assert_eq!(x.at.map(|a| a.qty), y.at.map(|a| a.qty));
    }
    // На стакане после покупки — другой итог (так считали старые записи).
    let late = Replay::new(
        &c.dir,
        &plan,
        a_after.book.clone(),
        Some(b.book.clone()),
        ["USDT", "USDT"],
    )
    .unwrap();
    let mut on_late = SourceCurve { cx: &c, src: &late };
    let l = uncapped(
        &mut on_late,
        &params(&rec, &a_after.book, &b.book),
        d("20"),
        &ladder,
        pass,
        None,
    )
    .await
    .unwrap();
    assert!(l.opt.qty != w.opt.qty || l.opt.profit != w.opt.profit);
}

#[test]
fn full_book_rules() {
    let (before, _) = frames();
    let t = Utc::now();
    let v = [VenueId("bingx".into())];
    // Решение 221: снимок торгового потока — раньше полного по изменениям (@incrDepth «all», вид массивом из
    // api-ai-skills), даже если изменения пришли позже снимка.
    let all = r#"{"code":0,"dataType":"CAKE-USDT@incrDepth","data":{"action":"all","lastUpdateId":7,"T":1,"bids":[["1.5","2"]],"asks":[["1.6","3"]]}}"#;
    let upd = r#"{"code":0,"dataType":"CAKE-USDT@incrDepth","data":{"action":"update","lastUpdateId":8,"T":2,"bids":[],"asks":[["1.7","4"]]}}"#;
    let snap_ns = ns(t) - 30;
    let rows = vec![
        line("bingx", "Book", snap_ns, &before),
        line("bingx", "Diff", ns(t) - 20, all),
        line("bingx", "Diff", ns(t) - 10, upd),
    ];
    let r = books_before(&rows, &v, &cake(), ns(t), &decode)
        .remove(&v[0])
        .unwrap()
        .unwrap();
    assert!(!r.full_depth, "сначала торговый снимок");
    assert_eq!(r.recv_ns, snap_ns);
    assert!(
        r.describe().contains("торгового потока"),
        "{}",
        r.describe()
    );
    // Торгового снимка нет — запасной: полный по изменениям.
    let r = books_before(&rows[1..], &v, &cake(), ns(t), &decode)
        .remove(&v[0])
        .unwrap()
        .unwrap();
    assert!(r.full_depth);
    assert_eq!(r.book.asks.len(), 2, "полный + применённое изменение");
    assert!(r.describe().contains("запасной"), "{}", r.describe());
    // Торговый снимок сброшен неприменимым кадром — запасной: полный по изменениям.
    let other = r#"{"code":0,"msg":"subscribed"}"#;
    let mut dropped = rows.clone();
    dropped.insert(1, line("bingx", "Book", ns(t) - 25, other));
    let r = books_before(&dropped, &v, &cake(), ns(t), &decode)
        .remove(&v[0])
        .unwrap()
        .unwrap();
    assert!(
        r.full_depth,
        "снимок сброшен — запасной стакан всей глубины"
    );
    // Только изменения без полного — стакана нет.
    let only = vec![line("bingx", "Diff", ns(t) - 10, upd)];
    let e = books_before(&only, &v, &cake(), ns(t), &decode)
        .remove(&v[0])
        .unwrap()
        .unwrap_err();
    assert!(e.contains("полного стакана"), "{e}");
    // Снимок, после которого в потоке стакана шло неприменимое (у Bybit — изменения), и полного по изменениям нет —
    // стакана нет.
    let rows = vec![
        line("bingx", "Book", ns(t) - 20, &before),
        line("bingx", "Book", ns(t) - 10, other),
    ];
    let e = books_before(&rows, &v, &cake(), ns(t), &decode)
        .remove(&v[0])
        .unwrap()
        .unwrap_err();
    assert!(e.contains("после снимка"), "{e}");
}

/// Настройки модели для сквозного пересчёта.
async fn settings(s: &Store) {
    let g = |n: &'static str, v: Value| (n, Scope::Global, v);
    let list = [
        g("card.max_search_usdt", json!(5000)),
        g("money.min_profit_usd", json!(0.01)),
        g("money.min_profit_pct", json!(0.01)),
        g("uncapped.max_evals", json!(300)),
        g("uncapped.ladder_usdt", json!(["50", "200"])),
        g("size.bisect_steps", json!(30)),
        g("size.grid_points", json!(20)),
        g("size.refine_steps", json!(30)),
        g("size.tolerance_pct", json!(0.1)),
        g("size.max_evals", json!(300)),
        g("profit.price_move_k", json!(1)),
    ];
    for (n, sc, v) in list {
        s.set(n, sc, Some(v), "тест").await.unwrap();
    }
    for v in ["bingx", "mexc", "bsc_dex"] {
        let sc = Scope::Venue(v.into());
        s.set("venue.quote_asset", sc.clone(), Some(json!("USDT")), "тест")
            .await
            .unwrap();
        s.set("profit.liquidity_share_h", sc, Some(json!(1)), "тест")
            .await
            .unwrap();
    }
}

/// Старая запись сделки (без поля basis — «после покупки»).
fn old_body(id: &str) -> String {
    json!({
        "battle": {"verdict": "deal", "reason": null, "deal_id": id, "dry": false, "qty": "30", "invested": "60",
                   "profit": "0.1", "budget": "100", "plan_budget": null, "free": null, "parts": null},
        "budget": "100", "opt": {"qty": "5", "invested": "10", "profit": "-0.03"}, "missed": "0",
    })
    .to_string()
}

#[tokio::test]
async fn recompute_end_to_end() {
    let dirp = tmp("e2e");
    let (core, s, db) = core(&dirp).await;
    settings(&s).await;
    let rec = Rec::load("recompute-e2e").await;
    let (before, after) = frames();
    let t = Utc::now();
    let w0 = t - Duration::seconds(1);
    // План — расчёт модели по снимку (как в бою), с площадками направления.
    let c = cx(&rec, "mexc");
    let book = |v: &str| {
        let mut b = books_before(
            &[line(v, "Book", ns(w0), &before)],
            &[VenueId(v.into())],
            &cake(),
            ns(t),
            &decode,
        );
        b.remove(&VenueId(v.into())).unwrap().unwrap().book
    };
    let fz = Frozen {
        a: Arc::new(frozen_side(&book("bingx"), Side::Buy)),
        b: Arc::new(frozen_side(&book("mexc"), Side::Sell)),
        at: t,
        decided: None,
    };
    let plan = serde_json::to_value(
        costs::evaluate_with(
            &c,
            d("30"),
            &Live {
                core: &core,
                src: Some(&fz),
            },
        )
        .await
        .unwrap(),
    )
    .unwrap();
    let deal = |id: &str, sell: &str, plan: Value| {
        let mut x = Deal::new(id.into(), dir(sell), plan, d("0.1"));
        x.timeline.push((t, "План: объём 30".into()));
        x
    };
    let win = |id: &str, deal: &str, venues: &[&str]| WindowInfo {
        id: id.into(),
        asset: cake(),
        dry: false,
        directions: vec![],
        venues: venues
            .iter()
            .map(|v| WinVenue {
                venue: VenueId((*v).into()),
                asset: cake(),
                since: w0,
                tap: None,
            })
            .collect(),
        started_at: w0,
        ended_at: None,
        max_profit: Decimal::ZERO,
        max_profit_unbounded: None,
        deals: vec![WinDeal {
            id: deal.into(),
            open: false,
            step: "Closed".into(),
            result_usd: None,
        }],
        outcome: None,
        frames: 0,
        lost: 0,
        bytes: 0,
        dex_points: 0,
        stopped: None,
        close_reason: None,
    };
    let file = |id: &str, rows: Vec<String>| {
        let z = zstd::encode_all(rows.join("\n").as_bytes(), 3).unwrap();
        std::fs::write(dirp.join("windows").join(format!("{id}.jsonl.zst")), z).unwrap();
    };
    file(
        "w1",
        vec![
            line("bingx", "Book", ns(w0), &before),
            line("mexc", "Book", ns(w0) + 1, &before),
            line("bingx", "Book", ns(t) + 1, &after),
        ],
    );
    file("w2", vec![line("bingx", "Book", ns(w0), &before)]);
    // Биржа → DEX: продажа на DEX из плана (маршрут есть — сторона DEX).
    let mut dex_plan = plan.clone();
    dex_plan["sell_quote"]["route"] =
        json!({"mode": "Direct", "legs": [], "block": 1, "gas_units": 1});
    dex_plan["sell_quote"]["venue"] = json!("bsc_dex");
    let mut back_plan = plan.clone();
    back_plan["buy_quote"]["route"] =
        json!({"mode": "Direct", "legs": [], "block": 1, "gas_units": 1});
    let windows = vec![
        win("w1", "D1", &["bingx", "mexc"]),
        win("w2", "D2", &["bingx"]),
    ];
    let env = Env {
        settings: &s,
        windows_dir: &dirp.join("windows"),
        decode: &decode,
    };
    let row = |n: i64, id: &str| DealRow {
        id: n,
        deal_id: id.into(),
        started_at: "2026-11-07T19:04:39Z".into(),
        body: old_body(id),
    };
    let cex = one(
        &env,
        &row(1, "D1"),
        Some(&deal("D1", "mexc", plan.clone())),
        &windows,
    )
    .await;
    assert_eq!(cex.line.group, G_CEX, "{:?}", cex.line.reason);
    let r = cex.row.unwrap();
    let body: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(body["basis"], BASIS_RECOMPUTED);
    assert_eq!(
        r.started_at, "2026-11-07T19:04:39Z",
        "начало — как у старой записи"
    );
    let brief: Brief = serde_json::from_str(&r.body).unwrap();
    assert_eq!(brief.decided_at, Some(t));
    assert!(!brief.understated());
    assert!(cex.line.source.contains("окно w1"));
    let dex = one(
        &env,
        &row(2, "D2"),
        Some(&deal("D2", "bsc_dex", dex_plan)),
        &windows,
    )
    .await;
    assert_eq!(dex.line.group, G_DEX, "{:?}", dex.line.reason);
    let dex_body: Value = serde_json::from_str(&dex.row.as_ref().unwrap().body).unwrap();
    assert_eq!(dex_body["basis"], BASIS_RECOMPUTED_DEX_PLAN);
    assert!(dex.line.source.contains("из плана"));
    let back = one(
        &env,
        &row(3, "D1"),
        Some(&deal("D1", "mexc", back_plan)),
        &windows,
    )
    .await;
    assert!(back.line.reason.unwrap().contains("DEX → биржа"));
    let none = one(
        &env,
        &row(4, "D3"),
        Some(&deal("D3", "mexc", plan.clone())),
        &windows,
    )
    .await;
    assert!(none.line.reason.unwrap().contains("окна у сделки нет"));
    // Запись: старая строка остаётся, новая — основная для сделки.
    let old_id = uncapped_db::insert_all(
        &db,
        vec![uncapped_db::Row {
            kind: "deal",
            deal_id: Some("D1".into()),
            ticker: "CAKE".into(),
            address: None,
            started_at: "2026-11-07T19:04:39Z".into(),
            body: old_body("D1"),
        }],
    )
    .await
    .unwrap()[0];
    let new_id = uncapped_db::insert_all(&db, vec![r]).await.unwrap()[0];
    assert_eq!(
        uncapped_db::by_deal(&db, "D1".into())
            .await
            .unwrap()
            .unwrap()
            .0,
        new_id
    );
    let all = uncapped_db::all(&db, Default::default()).await.unwrap();
    let kept: Vec<i64> = latest_per_deal(all).into_iter().map(|(id, _)| id).collect();
    assert_eq!(kept, vec![new_id]);
    assert!(old_id < new_id);
    // Бот держит базу (соединение открыто) — внешняя запись отказана, чтение — можно.
    let path = dirp.join("arb.sqlite");
    assert!(Db::open_existing(&path, true).is_err());
    assert!(Db::open_existing(&path, false).is_ok());
}

#[test]
fn telegram_and_report_mark_recomputed() {
    use arb::core::deal_ceiling::{Ceiling, parse};
    let body = |basis: &str| {
        json!({"opt": {"qty": "10", "invested": "100", "profit": "2"}, "opt_passes": true, "missed": "1", "basis": basis})
            .to_string()
    };
    let Ceiling::Known {
        recomputed,
        understated,
        ..
    } = parse(&body(BASIS_RECOMPUTED_DEX_PLAN))
    else {
        panic!("запись читается")
    };
    assert!(!understated);
    assert!(recomputed.unwrap().contains("DEX — из плана"));
    let Ceiling::Known { recomputed, .. } = parse(&body("decision_snapshot")) else {
        panic!("запись читается")
    };
    assert!(recomputed.is_none());
    assert!(
        arb::report::missed_labels::basis_label(BASIS_RECOMPUTED).contains("пересчитано по окну")
    );
}
