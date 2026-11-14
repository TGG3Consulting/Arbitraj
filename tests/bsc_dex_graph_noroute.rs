//! Файл: tests/bsc_dex_graph_noroute.rs
//! Что делает: проверяет решение 271 — монета, у которой пул ТОЛЬКО на форке (классического пути нет), при
//! dex.router_mode = graph получает исполнимую котировку графа из выбора маршрута (quote/routing.rs), а без graph —
//! прежнюю ошибку NoRoute классики. Данные — РЕАЛЬНЫЕ резервы записи routes_2026-10-29 (единственный V2-пул записи,
//! WBNB-USDT v2), как в tests/bsc_dex_biswap_graph.rs: меняется только МЕТКА семейства пула (v2_fork), к сети тест не
//! ходит. Утверждает: (1) graph + прямой режим и graph + умный режим — Ok, маршрут Graph, исполним, тот же, что даёт
//! graph_route::quote; (2) режим не задан / classic — NoRoute; (3) граф-маршрут неисполним (нет dex.bundle_max_routers)
//! — та же ошибка классики, текст в текст.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools::{families,registry}, quote::{routing,graph_route,steps,via}, parts},
//! tests/route_common (реальная запись и окружение), tests/route_common/fork.rs (сцена, routed).

mod route_common;

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{Quote, RouteMode, Side};
use arb::gateway::errors::ErrorCode;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::quote::graph_route;
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::Via;
use route_common::USDT;
use route_common::fork::{COIN, GAS, coin, routed, scene, venue, zero};

/// Котировка графа напрямую (graph_route::quote) — эталон для сравнения с выбором маршрута.
async fn graph_alone(p: &Parts, side: Side) -> Quote {
    let q = p.quoting();
    let taxes = zero();
    let v = Via {
        q: &q,
        coin: COIN,
        usdt: USDT,
        side,
        taxes: &taxes,
        coin_dec: 18,
        usdt_dec: 18,
    };
    let names = Names {
        usdt: (USDT, "USDT"),
        coin: (COIN, "COIN"),
        mids: &p.mids,
    };
    graph_route::quote(&v, &names, &coin(), Decimal::from(1000u32), U256::from(GAS))
        .await
        .expect("котировка графа")
}

/// Решение 271: пул только на форке, dex.router_mode = graph — выбор маршрута отдаёт исполнимую котировку графа
/// (обе стороны, прямой и умный режимы классики), ту же, что graph_route::quote.
#[tokio::test]
async fn fork_only_coin_quoted_by_graph() {
    let p = scene().await;
    venue(&p, "dex.router_mode", Some(json!("graph"))).await;
    for side in [Side::Buy, Side::Sell] {
        let want = graph_alone(&p, side).await;
        for smart in [false, true] {
            let got = routed(&p, side, smart)
                .await
                .unwrap_or_else(|e| panic!("{side:?} smart={smart}: {}", e.message));
            let r = got.route.as_ref().expect("маршрут");
            assert_eq!(r.mode, RouteMode::Graph, "{side:?} smart={smart}");
            assert!(r.executable, "{side:?} smart={smart}: {:?}", r.exec_note);
            assert_eq!(got.route, want.route, "{side:?} smart={smart}");
            assert_eq!(got.avg_price, want.avg_price, "{side:?} smart={smart}");
            assert!(got.signal_route.is_none());
        }
    }
}

/// Без graph (режим не задан / classic) — прежняя ошибка классики NoRoute; при graph с НЕисполнимым граф-маршрутом
/// (снят dex.bundle_max_routers) — та же ошибка, текст в текст.
#[tokio::test]
async fn fork_only_coin_no_route_without_executable_graph() {
    let p = scene().await;
    for smart in [false, true] {
        venue(&p, "dex.router_mode", None).await;
        venue(&p, "dex.bundle_max_routers", Some(json!(3))).await;
        let unset = routed(&p, Side::Buy, smart).await.expect_err("не задан");
        assert_eq!(unset.code, ErrorCode::NoRoute, "{}", unset.message);
        venue(&p, "dex.router_mode", Some(json!("classic"))).await;
        let classic = routed(&p, Side::Buy, smart).await.expect_err("classic");
        assert_eq!(classic.code, ErrorCode::NoRoute);
        assert_eq!(classic.message, unset.message);
        venue(&p, "dex.router_mode", Some(json!("graph"))).await;
        venue(&p, "dex.bundle_max_routers", None).await;
        assert!(!graph_alone(&p, Side::Buy).await.route.unwrap().executable);
        let graph = routed(&p, Side::Buy, smart).await.expect_err("сигнал");
        assert_eq!(graph.code, ErrorCode::NoRoute);
        assert_eq!(graph.message, unset.message, "текст ошибки классики");
    }
}
