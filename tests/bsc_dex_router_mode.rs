//! Файл: tests/bsc_dex_router_mode.rs
//! Что делает: проверяет врезку графового роутера (решение 80, PR 80.5a) на реальной записи CAKE 2026-10-29:
//! (1) графовый маршрут помечается RouteMode::Graph и флагом исполнимости как есть (DexRoute.executable, решение 84):
//!     кросс-DEX сплит — не исполним (сигнал), одиночный пул прямой формы — исполним; amountOutMin (выход доли) считается;
//! (2) выбор лучшего из классики и графа через pick (как в quote/mod.rs за dex.router_mode): граф не хуже классики.
//! Полный Quoting::quote узлозависим (цена газа, голова) и в юнит-тесте не поднимается; «classic бит-в-бит» сторожит
//! весь остальной набор тестов (router_mode там не задан → ветка графа не вызывается).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::{graph_route, direct_route, pick, via, steps}, tests/route_common.

mod route_common;

use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use rust_decimal::Decimal;

use arb::core::types::{Asset, Pool, PoolRef, PoolState, RouteMode, Side, TokenTaxes, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::graph::Edge;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::route_paths::{RoutePath, Step};
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::Via;
use arb::venues::bsc_dex::quote::{Quoting, direct_route, graph_route};

use route_common::{CAKE, USDT, WBNB};

const GAS: u64 = 1_000_000_000;

fn zero() -> TokenTaxes {
    TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    }
}

fn cake() -> Asset {
    Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    }
}

fn via<'a>(q: &'a Quoting, taxes: &'a TokenTaxes, side: Side) -> Via<'a> {
    Via {
        q,
        coin: CAKE,
        usdt: USDT,
        side,
        taxes,
        coin_dec: 18,
        usdt_dec: 18,
    }
}

fn names<'a>(q: &'a Quoting) -> Names<'a> {
    Names {
        usdt: (USDT, "USDT"),
        coin: (CAKE, "CAKE"),
        mids: &q.mids,
    }
}

#[tokio::test]
async fn graph_route_marks_mode_and_executability() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let t = zero();

    // Крупная покупка — кросс-DEX сплит: маршрут графа, не исполним как есть (решение 84), amountOutMin по ногам есть.
    let big = graph_route::quote(
        &via(&q, &t, Side::Buy),
        &names(&q),
        &cake(),
        Decimal::from(50_000u32),
        U256::from(GAS),
    )
    .await
    .expect("граф считает");
    let r = big.route.as_ref().unwrap();
    assert_eq!(r.mode, RouteMode::Graph);
    assert!(r.legs.len() >= 2, "сплит");
    assert!(
        !r.executable,
        "кросс-DEX сплит не исполним одной транзакцией"
    );
    assert!(r.exec_note.is_some(), "есть причина неисполнимости");
    assert!(
        r.legs.iter().all(|l| l.expected_out > Decimal::ZERO),
        "amountOutMin по ногам"
    );

    // Крошечная покупка — один пул прямой формы: исполним как есть, причины нет.
    let small = graph_route::quote(
        &via(&q, &t, Side::Buy),
        &names(&q),
        &cake(),
        Decimal::ONE,
        U256::from(GAS),
    )
    .await
    .expect("граф считает");
    let rs = small.route.as_ref().unwrap();
    assert_eq!(rs.mode, RouteMode::Graph);
    assert!(
        rs.executable && rs.exec_note.is_none(),
        "одиночный пул исполним как есть"
    );
}

async fn classic_direct(
    q: &Quoting,
    t: &TokenTaxes,
    side: Side,
    qty: Decimal,
) -> arb::core::types::Quote {
    let entries: Vec<Entry> = q
        .reg
        .by_pair(CAKE, USDT)
        .into_iter()
        .filter(|e| e.ticker == "CAKE")
        .collect();
    direct_route::quote(
        &via(q, t, side),
        &names(q),
        &cake(),
        qty,
        entries,
        &[],
        &|_: &Entry| true,
        U256::from(GAS),
    )
    .await
    .expect("классика считает")
}

#[tokio::test]
async fn merge_trades_best_executable_never_worse_than_classic() {
    // Решение 84 вариант Б: на сделку идёт лучший ИСПОЛНИМЫЙ маршрут. Лучший по графу (кросс-DEX сплит) неисполним
    // как есть — на сделку не берётся, котировкой остаётся исполнимая классика; graph-режим не хуже классики.
    let p = route_common::parts().await;
    let q = p.quoting();
    let t = zero();
    let (side, qty) = (Side::Buy, Decimal::from(50_000u32));
    let classic = classic_direct(&q, &t, side, qty).await;
    let graph = graph_route::quote(
        &via(&q, &t, side),
        &names(&q),
        &cake(),
        qty,
        U256::from(GAS),
    )
    .await
    .expect("граф считает");
    assert!(
        !graph.route.as_ref().unwrap().executable,
        "кросс-DEX сплит неисполним как есть"
    );

    let merged = graph_route::merge(&q, classic.clone(), Ok(graph), side, 18).await;
    assert_eq!(
        merged.route.as_ref().unwrap().mode,
        RouteMode::Direct,
        "на сделку — исполнимая классика"
    );
    assert_eq!(
        merged.avg_price, classic.avg_price,
        "не хуже классики (ровно классика)"
    );
    // Лучший найденный (неисполнимый) граф-маршрут донесён как сигнал — для карточки (решение 84 вариант Б).
    let sig = merged
        .signal_route
        .as_ref()
        .expect("лучший граф-маршрут донесён как signal_route");
    assert_eq!(sig.mode, RouteMode::Graph);
    assert!(
        !sig.executable && sig.legs.len() >= 2,
        "сигнал — неисполнимый кросс-DEX сплит"
    );

    // Сбой графа — откат к классике (merge логирует и возвращает классику).
    let err = q.ctx.err(ErrorCode::NoRoute, "тест: граф без маршрута");
    let fallback = graph_route::merge(&q, classic.clone(), Err(err), side, 18).await;
    assert_eq!(fallback.avg_price, classic.avg_price);
    assert_eq!(fallback.route.as_ref().unwrap().mode, RouteMode::Direct);
}

#[tokio::test]
async fn merge_keeps_executable_graph() {
    // Исполнимый графовый маршрут (крошечный объём — один пул) участвует в выборе: итог исполним.
    let p = route_common::parts().await;
    let q = p.quoting();
    let t = zero();
    let (side, qty) = (Side::Buy, Decimal::ONE);
    let classic = classic_direct(&q, &t, side, qty).await;
    let graph = graph_route::quote(
        &via(&q, &t, side),
        &names(&q),
        &cake(),
        qty,
        U256::from(GAS),
    )
    .await
    .expect("граф считает");
    assert!(
        graph.route.as_ref().unwrap().executable,
        "один пул исполним как есть"
    );
    let merged = graph_route::merge(&q, classic, Ok(graph), side, 18).await;
    assert!(
        merged.route.as_ref().unwrap().executable,
        "итог сделки исполним"
    );
}

/// Ребро заданного вида между токенами (для проверки фильтра решения 62; важен только pool.kind).
fn edge(kind: &str, x: Address, y: Address) -> Edge {
    let f = families::make(kind, kind).unwrap();
    let (t0, t1) = if x < y { (x, y) } else { (y, x) };
    let entry = Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(address!("0x00000000000000000000000000000000000000ff")),
            token0: t0,
            token1: t1,
            fee_pips: Some(2500),
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::V2 {
                reserve0: U256::ZERO,
                reserve1: U256::ZERO,
            },
            block: 1,
            received_at: Utc::now(),
        },
        ticker: "X".into(),
        coin: x,
        usdt: y,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
        asleep: false,
    };
    Edge { entry, a: x, b: y }
}

#[tokio::test]
async fn native_route_excludes_v4_infinity() {
    // Решение 62: нативную монету (WBNB) граф не маршрутизирует через V4 / Infinity (CL, Bin) — только V2/V3.
    let p1 = RoutePath {
        steps: vec![Step {
            edge: edge("pancakeswap_v2", WBNB, USDT),
            token_in: WBNB,
            token_out: USDT,
        }],
    };
    assert!(
        graph_route::native_ok(WBNB, &p1),
        "V2-пул на нативной монете — ок"
    );

    let p2 = RoutePath {
        steps: vec![Step {
            edge: edge("pancakeswap_infinity_cl", WBNB, USDT),
            token_in: WBNB,
            token_out: USDT,
        }],
    };
    assert!(
        !graph_route::native_ok(WBNB, &p2),
        "Infinity CL на нативной монете — отсечь (решение 62)"
    );

    // CL-пул, не касающийся нативной монеты, ограничение не затрагивает.
    let p3 = RoutePath {
        steps: vec![Step {
            edge: edge("pancakeswap_infinity_cl", CAKE, USDT),
            token_in: CAKE,
            token_out: USDT,
        }],
    };
    assert!(
        graph_route::native_ok(WBNB, &p3),
        "CL не на нативной монете — ок"
    );
}
