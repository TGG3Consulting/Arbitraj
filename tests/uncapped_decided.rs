//! Файл: tests/uncapped_decided.rs
//! Что проверяет: решение 237 — расчёт «без потолка» по сделке целиком по данным момента входа. Живое ядро с SQLite
//! (настоящий Core, настройки в хранилище), данные — записанный стакан BingX CAKE-USDT@depth20
//! (tests/data/bingx/ws_spot_2026-10-23.jsonl), правила пары и сеть BEP20 CAKE у BingX (tests/size_common). Вторая
//! биржа — id mexc с тем же стаканом BingX (записанных стаканов MEXC в репозитории нет, как в tests/uncapped_recompute.rs).
//! 1) core/uncapped.rs compute считает по контексту момента решения (Frozen.decided): σ плана, а не σ, которая была бы
//!    «живой» при расчёте (сделка INTCB 08.11: σ выросла в ~2 раза за 4.9 с, оптимум съехал 450 → 340.81). Итог
//!    compute совпадает с поиском size_uncapped по той же кривой с σ плана и отличается от поиска с удвоенной σ.
//! 2) Объём, на котором вошли, посчитан всегда — даже при исчерпанном uncapped.max_evals («на нашем бюджете» у сделки —
//!    точка на этом объёме), упущено = прибыль на потолке − на вложенном (одна модель).
//! 3) Курсы затрат — только из плана сделки: валюты нет в плане — ошибка, живой курс не подставляется.
//! 4) Снимок без входов момента решения — расчёта по сделке нет (причина).
//! 5) Решение 239: настройки модели (пороги money.min_profit_*, uncapped.*, size.*, card.max_search_usdt) — из копии на
//!    момент входа (Decided.settings, settings/pinned.rs): изменение настроек после входа расчёт по сделке не меняет;
//!    копия только для чтения.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{uncapped, frozen, plan_costs, costs, size_uncapped, size_curve}, tests/size_common.

mod size_common;

use std::sync::Arc;

use arb::core::Core;
use arb::core::costs::{self, Live, Source};
use arb::core::deps::Deps;
use arb::core::dirctx::DirCtx;
use arb::core::frozen::{Decided, Frozen};
use arb::core::plan_costs::PlanCosts;
use arb::core::registry::Presence;
use arb::core::size_curve::{SourceCurve, params};
use arb::core::size_uncapped::uncapped;
use arb::core::types::{Asset, Cost, CostKind, Direction, OrderBook, Side, VenueId};
use arb::core::uncapped_types::{BASIS_DECISION, Battle, missed};
use arb::gateway::frozen::{BookFee, FrozenBook};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use size_common::{CAKE, Rec, USDT, d};
use tokio::sync::broadcast;

fn cake() -> Asset {
    Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    }
}

fn dir() -> Direction {
    Direction {
        asset: cake(),
        buy_on: VenueId("bingx".into()),
        sell_on: VenueId("mexc".into()),
    }
}

/// Записанный стакан CAKE (площадка v).
fn book(rec: &Rec, v: &str) -> OrderBook {
    OrderBook {
        venue: VenueId(v.into()),
        asset: cake(),
        bids: rec.bids.clone(),
        asks: rec.asks.clone(),
        exchange_time: None,
        received_at: Utc::now(),
    }
}

/// Контекст направления на записанных данных; σ — заданная.
fn cx(rec: &Rec, sigma: Decimal) -> DirCtx {
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
        dir: dir(),
        a: p("bingx"),
        b: p("mexc"),
        net_a: "BEP20".into(),
        net_b: "BEP20".into(),
        quote: vec!["USDT".into(), "USDT".into()],
        usdt: Asset {
            ticker: "USDT".into(),
            bsc_contract: Some(USDT),
        },
        cex: vec![],
        sigma,
        transit_s: d("120"),
        k: d("1"),
        h: d("1"),
        return_per_trade: false,
        pair_a: Some(rec.pair.clone()),
        pair_b: Some(rec.pair.clone()),
        usdt_b: None,
        transfer_tax: Some(Decimal::ZERO),
    }
}

/// Снимок стороны (как адаптер биржи в момент решения): покупка — комиссия в монете, продажа — от суммы.
fn side(b: &OrderBook, s: Side) -> FrozenBook {
    let (per_coin, asset) = match s {
        Side::Buy => (true, "CAKE"),
        Side::Sell => (false, "USDT"),
    };
    FrozenBook {
        venue: b.venue.clone(),
        asset: cake(),
        side: s,
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

/// Настоящее ядро без площадок (как tests/uncapped_recompute.rs).
async fn core(dir: &std::path::Path) -> (Core, Store) {
    let db = Db::open(&dir.join("arb.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(dir.join("secrets.enc")),
        journal: journal.clone(),
    };
    let deps = Deps {
        gw: Registry::new(),
        settings: settings.clone(),
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: dir.to_path_buf(),
    };
    (Core::start(deps).await, settings)
}

/// Настройки модели в хранилище (uncapped.max_evals — заданный).
async fn settings(s: &Store, max_evals: i64) {
    let g = |n: &'static str, v: Value| (n, Scope::Global, v);
    let list = [
        g("card.max_search_usdt", json!(5000)),
        g("money.min_profit_usd", json!(0.01)),
        g("money.min_profit_pct", json!(0.01)),
        g("uncapped.max_evals", json!(max_evals)),
        g("uncapped.ladder_usdt", json!(["50", "200"])),
        g("size.bisect_steps", json!(30)),
        g("size.grid_points", json!(20)),
        g("size.refine_steps", json!(30)),
        g("size.tolerance_pct", json!(0.1)),
        g("size.max_evals", json!(300)),
    ];
    for (n, sc, v) in list {
        s.set(n, sc, Some(v), "тест").await.unwrap();
    }
}

fn tmp(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("arb_decided_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Снимок с входами момента решения: контекст с σ, затраты плана и копия настроек хранилища `s` на этот момент.
fn frozen(rec: &Rec, sigma: Decimal, plan: &costs::Calc, s: &Store) -> Frozen {
    let lines = PlanCosts::lines_of(&plan.costs);
    Frozen {
        a: Arc::new(side(&book(rec, "bingx"), Side::Buy)),
        b: Arc::new(side(&book(rec, "mexc"), Side::Sell)),
        at: Utc::now(),
        decided: Some(Arc::new(Decided {
            cx: cx(rec, sigma),
            plan: PlanCosts::new("CAKE", &lines, plan.arrive),
            settings: s.pinned(),
        })),
    }
}

/// Бой сделки: вошли на плановом объёме.
fn battle(plan: &costs::Calc) -> Battle {
    Battle {
        verdict: "deal".into(),
        reason: None,
        deal_id: Some("20261109000000000CAKE".into()),
        dry: false,
        qty: plan.qty,
        invested: plan.spend_max,
        profit: plan.profit_usd,
        budget: Some(d("40")),
        plan_budget: Some(d("40")),
        free: None,
        parts: None,
        window_from: None,
    }
}

/// План сделки — расчёт той же модели по снимку на объёме qty с σ плана.
async fn plan(core: &Core, rec: &Rec, sigma: Decimal, qty: Decimal) -> costs::Calc {
    let raw = Frozen {
        a: Arc::new(side(&book(rec, "bingx"), Side::Buy)),
        b: Arc::new(side(&book(rec, "mexc"), Side::Sell)),
        at: Utc::now(),
        decided: None,
    };
    let live = Live {
        core,
        src: Some(&raw),
    };
    costs::evaluate_with(&cx(rec, sigma), qty, &live)
        .await
        .unwrap()
}

#[tokio::test]
async fn compute_uses_plan_sigma() {
    let p = tmp("sigma");
    let (core, s) = core(&p).await;
    settings(&s, 300).await;
    let rec = Rec::load("decided-sigma").await;
    let sigma = d("0.0005");
    let pl = plan(&core, &rec, sigma, d("3")).await;
    let fz = frozen(&rec, sigma, &pl, &s);
    let r = arb::core::uncapped::compute(&core, &dir(), battle(&pl), Some(Ok(fz.clone())))
        .await
        .unwrap();
    assert_eq!(r.basis, BASIS_DECISION);
    assert_eq!(r.opt_transit_s, d("120"));
    // Тот же поиск по той же кривой с σ плана (источник Live со снимком и входами) — тот же итог.
    let live = Live {
        core: &core,
        src: Some(&fz),
    };
    let c = cx(&rec, sigma);
    let price = fz.book(false).unwrap().asks[0].price;
    let mut pr = params(&core, &c, d("5000"), price, Some(&fz)).unwrap();
    pr.max_evals = 300;
    let ladder = [d("50"), d("200")];
    let pass = |pt: &arb::core::size::Point<costs::Calc>| {
        arb::core::card_max::passes(&pt.data, d("0.01"), d("0.01"))
    };
    let mut curve = SourceCurve { cx: &c, src: &live };
    let u = uncapped(&mut curve, &pr, d("40"), &ladder, pass, Some(pl.qty))
        .await
        .unwrap();
    assert_eq!((r.opt.qty, r.opt.profit), (u.opt.qty, u.opt.profit));
    // σ, которая была бы «живой» при расчёте (вдвое больше), — другой итог: compute её не берёт.
    let c2 = cx(&rec, sigma * d("2"));
    let mut curve2 = SourceCurve {
        cx: &c2,
        src: &live,
    };
    let u2 = uncapped(&mut curve2, &pr, d("40"), &ladder, pass, Some(pl.qty))
        .await
        .unwrap();
    assert!(
        u2.opt.profit < u.opt.profit,
        "{} vs {}",
        u2.opt.profit,
        u.opt.profit
    );
    // Точка на вложенном объёме — тот же расчёт, что план (одна модель, одни данные).
    let at = r.at_budget.at.expect("точка на вложенном объёме");
    assert_eq!(at.qty, pl.qty);
    assert_eq!(at.profit, pl.profit_usd);
    assert_eq!(
        r.missed,
        missed("deal", r.opt_passes, r.opt.profit, &r.at_budget)
    );
}

#[tokio::test]
async fn invested_point_beyond_max_evals() {
    let p = tmp("evals");
    let (core, s) = core(&p).await;
    // Предел расчётов меньше, чем нужно поиску (на этой записи: 40 — поиск не находит границ, 130 — не исчерпан, 60 —
    // поиск закончен, предел исчерпан, как у INTCB: 160 из 160): точка вложенного объёма всё равно считается.
    settings(&s, 60).await;
    let rec = Rec::load("decided-evals").await;
    let sigma = d("0.0005");
    let pl = plan(&core, &rec, sigma, d("3.7")).await;
    let fz = frozen(&rec, sigma, &pl, &s);
    let r = arb::core::uncapped::compute(&core, &dir(), battle(&pl), Some(Ok(fz.clone())))
        .await
        .unwrap();
    assert!(r.capped, "предел исчерпан");
    let at = r
        .at_budget
        .at
        .expect("точка на вложенном объёме посчитана вне предела");
    assert_eq!((at.qty, at.profit), (pl.qty, pl.profit_usd));
    // Без объёма сделки (как до решения 237) «на бюджете» — лучшая посчитанная в бюджете, не вложенный объём.
    let live = Live {
        core: &core,
        src: Some(&fz),
    };
    let c = cx(&rec, sigma);
    let price = fz.book(false).unwrap().asks[0].price;
    let mut pr = params(&core, &c, d("5000"), price, Some(&fz)).unwrap();
    pr.max_evals = 60;
    let mut curve = SourceCurve { cx: &c, src: &live };
    let old = uncapped(&mut curve, &pr, d("40"), &[], |_| true, None)
        .await
        .unwrap();
    assert_ne!(old.at_budget.at.map(|a| a.qty), Some(pl.qty));
}

#[tokio::test]
async fn no_live_fallback() {
    let p = tmp("live");
    let (core, s) = core(&p).await;
    settings(&s, 300).await;
    let rec = Rec::load("decided-live").await;
    let sigma = d("0.0005");
    let pl = plan(&core, &rec, sigma, d("3")).await;
    let fz = frozen(&rec, sigma, &pl, &s);
    // Курс валюты, которой нет в плане (BNB), — ошибка, а не живой курс.
    let live = Live {
        core: &core,
        src: Some(&fz),
    };
    let gas = Cost::new(CostKind::Gas, d("0.001"), "BNB".to_string(), false);
    let e = live
        .rates(&cx(&rec, sigma), "CAKE", d("2"), &[gas])
        .await
        .err()
        .unwrap();
    assert!(e.contains("курса в плане сделки нет"), "{e}");
    // Снимок без входов момента решения — расчёта по сделке нет.
    let raw = Frozen {
        decided: None,
        ..frozen(&rec, sigma, &pl, &s)
    };
    let e = arb::core::uncapped::compute(&core, &dir(), battle(&pl), Some(Ok(raw)))
        .await
        .err()
        .unwrap();
    assert!(
        e.contains("входы расчёта на момент решения не сняты"),
        "{e}"
    );
}

#[tokio::test]
async fn settings_at_entry() {
    let p = tmp("settings");
    let (core, s) = core(&p).await;
    settings(&s, 300).await;
    let rec = Rec::load("decided-settings").await;
    let sigma = d("0.0005");
    let pl = plan(&core, &rec, sigma, d("3")).await;
    let fz = frozen(&rec, sigma, &pl, &s);
    let before = arb::core::uncapped::compute(&core, &dir(), battle(&pl), Some(Ok(fz.clone())))
        .await
        .unwrap();
    // После входа Тигран меняет настройки модели: порог прибыли недостижимый, другая таблица потолков, меньше
    // предел поиска и расчётов.
    for (n, v) in [
        ("money.min_profit_usd", json!(1_000_000)),
        ("uncapped.ladder_usdt", json!(["10"])),
        ("card.max_search_usdt", json!(30)),
        ("uncapped.max_evals", json!(5)),
        ("size.grid_points", json!(3)),
    ] {
        s.set(n, Scope::Global, Some(v), "тест").await.unwrap();
    }
    // Расчёт по сделке — по настройкам момента входа: тот же итог.
    let after = arb::core::uncapped::compute(&core, &dir(), battle(&pl), Some(Ok(fz.clone())))
        .await
        .unwrap();
    assert_eq!(after.min_profit_usd, d("0.01"));
    assert_eq!(after.search_cap, d("5000"));
    assert_eq!(
        (after.opt.qty, after.opt.profit),
        (before.opt.qty, before.opt.profit)
    );
    assert_eq!(after.opt_passes, before.opt_passes);
    assert_eq!(after.levels, before.levels);
    assert_eq!(after.at_budget, before.at_budget);
    // Снимок, взятый после изменения, — уже по новым настройкам: предел расчётов 5 исчерпан (у снимка момента входа —
    // 300, расчёт выше прошёл).
    let late = arb::core::uncapped::compute(
        &core,
        &dir(),
        battle(&pl),
        Some(Ok(frozen(&rec, sigma, &pl, &s))),
    )
    .await
    .unwrap_err();
    assert!(late.contains("= 5"), "{late}");
    // Копия — только для чтения.
    let pinned = fz.decided.as_ref().unwrap().settings.clone();
    let e = pinned
        .set(
            "money.min_profit_usd",
            Scope::Global,
            Some(json!(1)),
            "тест",
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("копия настроек"), "{e}");
}
