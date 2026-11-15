//! Файл: tests/bsc_dex_fit_graph_294.rs
//! Что проверяет: решение 294 в режиме dex.router_mode = graph (так на сервере 15.11, dex.swap_mode = direct) на РЕАЛЬНОЙ
//! записи цепи tests/data/fit_volume_2026-11-15 (пул PancakeSwap V3 SHOPINX/WBNB, пара PancakeSwap V2 WBNB/USDT,
//! блок 133732042; карта ±2 слова). Шаг сплита smart.split_step_pct — 5 (значение сервера) и 10.
//! 57000 SHOPINX не помещаются в путь через WBNB:
//! - граф исполним (канал bundle + исполнитель кошелька проб): без dex.intermediate_partial_fill и с «нет» — часть
//!   графа (решение 271), 1 в 1; «да» — точная часть классики (больше части графа по шагу сплита);
//! - граф неисполним (канала нет): без настройки и «нет» — прежний NoRoute; «да» — классика;
//! - граф с ошибкой (dex.route_max_hops снята): «да» — классика, записей журнала «dex.router_graph» не прибавилось;
//! - 1000 SHOPINX (помещается): 1 в 1 с настройкой и без, в обоих режимах;
//! - финальная котировка (gateway::quote_stage::final_scope) — тот же итог (адаптер ветвится по ней только при
//!   exec.via_kyber, venue.rs).
//! Записано с цепи — пулы; задано разработчиком — карта ±2 слова, объёмы, газ 1 gwei, налоги 0, канал и адрес
//! исполнителя (условные: к сети тест не ходит; исполнимость графа читает только их наличие).
//! Кто вызывает: cargo test.
//! От чего зависит: tests/fit_common, tests/route_common, arb::venues::bsc_dex::quote::{routing, graph_route}.

mod fit_common;
mod route_common;

use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{RouteMode, Side};
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use fit_common::{d, full, js, partial, scene};

/// Помещающаяся часть классики (точный расчёт, tests/core_fit_volume_294.rs).
fn fit(side: Side) -> Decimal {
    match side {
        Side::Sell => d("29666.114904110584943289"),
        Side::Buy => d("28380.895110992572453266"),
    }
}

/// Настройка площадки bsc_dex (None — снять). Ошибок нет.
async fn set(p: &Parts, n: &str, v: Option<serde_json::Value>) {
    let s = Scope::Venue("bsc_dex".into());
    p.ctx.settings.set(n, s, v, "тест").await.unwrap();
}

/// Сцена режима graph с шагом сплита pct; exec — канал bundle и исполнитель кошелька проб. Ошибок нет.
async fn graph_scene(pct: &str, exec: bool) -> Parts {
    let p = scene().await;
    set(&p, "dex.router_mode", Some(json!("graph"))).await;
    set(&p, "smart.split_step_pct", Some(json!(pct))).await;
    if exec {
        route_common::channel(&p, "club48", "bundle").await;
        route_common::executor(&p, "0x0000000000000000000000000000000000000077").await;
    }
    p
}

/// Сколько записей журнала вида «dex.router_graph». Ошибок нет.
async fn graph_warns(p: &Parts) -> usize {
    let all = p.ctx.journal.list(10_000, None).await.unwrap();
    all.iter().filter(|e| e.kind == "dex.router_graph").count()
}

#[tokio::test]
async fn executable_graph_vs_exact_classic() {
    for pct in ["5", "10"] {
        let p = graph_scene(pct, true).await;
        let q = p.quoting();
        for side in [Side::Sell, Side::Buy] {
            let w0 = graph_warns(&p).await;
            partial(&p, None).await;
            let off = full(&q, side, d("57000")).await;
            partial(&p, Some(false)).await;
            let no = full(&q, side, d("57000")).await;
            assert_eq!(js(&off), js(&no), "pct={pct} {side:?}");
            let g = off.unwrap();
            assert_eq!(
                g.route.as_ref().map(|r| (r.mode, r.executable)),
                Some((RouteMode::Graph, true))
            );
            partial(&p, Some(true)).await;
            let on = full(&q, side, d("57000")).await.unwrap();
            assert_eq!(on.fillable_qty, fit(side), "pct={pct} {side:?}");
            assert!(on.fillable_qty > g.fillable_qty);
            assert_eq!(
                on.route.as_ref().map(|r| (r.mode, r.executable)),
                Some((RouteMode::Direct, true))
            );
            assert_eq!(
                graph_warns(&p).await,
                w0,
                "записей журнала графа не прибавилось"
            );
            println!(
                "pct={pct} {side:?}: граф {} → классика {}",
                g.fillable_qty, on.fillable_qty
            );
        }
    }
}

#[tokio::test]
async fn unexecutable_graph_gives_classic() {
    for pct in ["5", "10"] {
        let p = graph_scene(pct, false).await;
        let q = p.quoting();
        for side in [Side::Sell, Side::Buy] {
            partial(&p, None).await;
            let off = full(&q, side, d("57000")).await;
            partial(&p, Some(false)).await;
            let no = full(&q, side, d("57000")).await;
            assert_eq!(js(&off), js(&no));
            let e = off.unwrap_err();
            assert_eq!(e.code, ErrorCode::NoRoute);
            assert!(e.message.contains("не исполним целиком"), "{}", e.message);
            partial(&p, Some(true)).await;
            let on = full(&q, side, d("57000")).await.unwrap();
            assert_eq!(on.fillable_qty, fit(side));
            assert_eq!(on.route.as_ref().map(|r| r.mode), Some(RouteMode::Direct));
        }
    }
}

#[tokio::test]
async fn graph_error_gives_classic_without_warn() {
    let p = graph_scene("5", true).await;
    set(&p, "dex.route_max_hops", None).await;
    let q = p.quoting();
    for side in [Side::Sell, Side::Buy] {
        let w0 = graph_warns(&p).await;
        partial(&p, None).await;
        let off = full(&q, side, d("57000")).await.unwrap_err();
        assert_eq!(off.code, ErrorCode::NoRoute);
        partial(&p, Some(true)).await;
        let on = full(&q, side, d("57000")).await.unwrap();
        assert_eq!(on.fillable_qty, fit(side));
        assert_eq!(graph_warns(&p).await, w0);
    }
    // Объём помещается, граф с ошибкой — прежний merge: запись журнала графа есть (с настройкой и без одинаково).
    for on in [None, Some(true)] {
        partial(&p, on).await;
        let w0 = graph_warns(&p).await;
        let r = full(&q, Side::Sell, d("1000")).await.unwrap();
        assert_eq!(r.fillable_qty, d("1000"));
        assert_eq!(graph_warns(&p).await, w0 + 1, "{on:?}");
    }
}

#[tokio::test]
async fn fitting_volume_and_final_scope_are_unchanged() {
    for (mode, exec) in [("classic", false), ("graph", false), ("graph", true)] {
        let p = graph_scene("5", exec).await;
        set(&p, "dex.router_mode", Some(json!(mode))).await;
        let q = p.quoting();
        for side in [Side::Sell, Side::Buy] {
            partial(&p, None).await;
            let a = full(&q, side, d("1000")).await;
            partial(&p, Some(true)).await;
            let b = full(&q, side, d("1000")).await;
            assert_eq!(js(&a), js(&b), "{mode} {exec} {side:?}");
            // Финальная котировка — тот же код выбора маршрута.
            let f = arb::gateway::quote_stage::final_scope(full(&q, side, d("57000"))).await;
            let n = full(&q, side, d("57000")).await;
            assert_eq!(js(&f), js(&n), "{mode} {exec} {side:?}");
        }
    }
}
