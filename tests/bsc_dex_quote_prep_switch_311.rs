//! Файл: tests/bsc_dex_quote_prep_switch_311.rs
//! Что делает: решение 311 А, выключатель dex.quote_prep_once (Тигран 16.11.2026: «сделай вкл выкл. по дефолту вкл»)
//! на записи маршрута CAKE (tests/data/routes_2026-10-29, route_common: шесть настоящих пулов) в режиме
//! dex.router_mode = graph, путь котировки как в tests/bsc_dex_quote_timing_312.rs (cpu::run(timed(routing::run))).
//! Котировка при «не задана», «нет» и «да» — та же (объём, средняя цена, маршрут); перед каждой слоты книги глубины
//! помечаются устаревшими (Book::invalidate через reg.depth()), чтобы глубина считалась заново в каждом варианте.
//! Общая подготовка глубины проверяется без таймингов — по счётчику этапа prep замера решения 312 (log.quote_timing = да,
//! порог 0): не задана и «да» — prep_n ≥ 1 (одна Prep на котировку), «нет» — prep_n = 0 (Prep не создаётся, каждый хоп
//! готовит сам), допуск хопов (legs_n) идёт во всех вариантах. Конкретные миллисекунды не проверяются.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::{cpu, routing, quote_timing, quote_timing_outer}, tests/route_common.

mod route_common;

use std::time::Duration;

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::types::{Asset, Quote, Side, TokenTaxes};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::quote::quote_timing;
use arb::venues::bsc_dex::quote::quote_timing_outer::{Outcome, Outer};
use arb::venues::bsc_dex::quote::{Quoting, cpu, routing};

use route_common::{CAKE, USDT};

const KIND: &str = "dex.quote_timing";
const SWITCH: &str = "dex.quote_prep_once";

/// Задаёт настройку `n` (None — снять) в области `scope`.
async fn set(p: &Parts, n: &str, scope: Scope, val: Option<Value>) {
    p.ctx.settings.set(n, scope, val, "тест").await.unwrap();
}

/// Окружение записи маршрута в режиме графа с замером каждой котировки (порог 0).
async fn parts() -> Parts {
    let p = route_common::parts().await;
    let v = Scope::Venue("bsc_dex".into());
    set(&p, "dex.router_mode", v.clone(), Some(json!("graph"))).await;
    set(&p, "dex.swap_mode", v, Some(json!("direct"))).await;
    set(&p, "log.quote_timing", Scope::Global, Some(json!(true))).await;
    set(&p, "log.quote_timing_min_ms", Scope::Global, Some(json!(0))).await;
    p
}

/// Входные данные выбора маршрута для покупки CAKE (как их собирает quote/mod.rs).
fn input(q: &Quoting, qty: u32) -> routing::Input {
    let entries = q
        .reg
        .by_pair(CAKE, USDT)
        .into_iter()
        .filter(|e| e.ticker == "CAKE")
        .collect();
    let mids = q.mids.active(&q.ctx).into_iter().map(|m| (m, 18)).collect();
    routing::Input {
        asset: Asset {
            chain: None,
            ticker: "CAKE".into(),
            contract: Some(CAKE),
        },
        side: Side::Buy,
        qty: Decimal::from(qty),
        coin: CAKE,
        usdt: USDT,
        taxes: TokenTaxes {
            buy_pct: Decimal::ZERO,
            sell_pct: Decimal::ZERO,
            transfer_pct: Decimal::ZERO,
        },
        coin_dec: 18,
        usdt_dec: 18,
        entries,
        mids,
        gas: U256::from(1_000_000_000u64),
        usdt_ticker: "USDT".into(),
        smart: false,
    }
}

/// Записи dex.quote_timing в журнале.
async fn entries(q: &Quoting) -> Vec<Value> {
    let all = q.ctx.journal.list(1000, None).await.unwrap();
    all.into_iter()
        .filter(|e| e.kind == KIND)
        .filter_map(|e| e.data)
        .collect()
}

/// Котировка с замером (путь Quoting::priced) после пометки слотов глубины устаревшими. Возвращает котировку и data её
/// записи dex.quote_timing (новая запись среди уже бывших; ожидание ограничено).
async fn quote(q: &Quoting) -> (Quote, Value) {
    let before = entries(q).await;
    q.reg.depth().invalidate();
    let mut tm = Outer::begin(&q.ctx, "CAKE", Side::Buy, Decimal::from(5_000u32));
    let work = quote_timing::timed(tm.run(), routing::run(q.clone(), input(q, 5_000)));
    let out = cpu::run(&q.ctx, Box::pin(work)).await;
    tm.back();
    tm.finish(|| Outcome::of(&out));
    let quote = out.expect("котировка");
    for _ in 0..200 {
        let got = entries(q).await;
        if let Some(d) = got.into_iter().find(|d| !before.contains(d)) {
            return (quote, d);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("нет записи замера котировки");
}

/// Число из data (поле обязано быть числом).
fn num(d: &Value, key: &str) -> f64 {
    d[key]
        .as_f64()
        .unwrap_or_else(|| panic!("поле {key} не число: {d}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn switch_off_same_quote_no_shared_prep() {
    let p = parts().await;
    let q = p.quoting();
    let v = Scope::Venue("bsc_dex".into());
    // Не задана — включено: одна подготовка на котировку.
    let (unset, d) = quote(&q).await;
    assert!(num(&d, "prep_n") >= 1.0, "не задана — общая Prep: {d}");
    assert!(num(&d, "legs_n") >= 1.0);
    // «Нет» — как до 311 А: общей подготовки нет, хопы готовят сами.
    set(&p, SWITCH, v.clone(), Some(json!(false))).await;
    let (off, d) = quote(&q).await;
    assert_eq!(num(&d, "prep_n"), 0.0, "«нет» — общей Prep нет: {d}");
    assert!(num(&d, "legs_n") >= 1.0);
    // «Да» — снова одна подготовка на котировку.
    set(&p, SWITCH, v, Some(json!(true))).await;
    let (on, d) = quote(&q).await;
    assert!(num(&d, "prep_n") >= 1.0, "«да» — общая Prep: {d}");
    // Котировка одна и та же во всех трёх вариантах.
    for (name, x) in [("нет", &off), ("да", &on)] {
        assert_eq!(x.fillable_qty, unset.fillable_qty, "объём, {name}");
        assert_eq!(x.avg_price, unset.avg_price, "средняя цена, {name}");
        assert_eq!(x.route, unset.route, "маршрут, {name}");
    }
}
