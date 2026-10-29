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

use alloy_primitives::U256;
use rust_decimal::Decimal;

use arb::core::types::{Asset, RouteMode, Side, TokenTaxes};
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::Via;
use arb::venues::bsc_dex::quote::{Quoting, direct_route, graph_route, pick};

use route_common::{CAKE, USDT};

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

#[tokio::test]
async fn switch_picks_best_graph_not_worse_than_classic() {
    // Как врезка в quote/mod.rs: классика (прямой лучший пул) против графа через pick — лучший выигрывает.
    let p = route_common::parts().await;
    let q = p.quoting();
    let t = zero();
    let qty = Decimal::from(50_000u32);
    let side = Side::Buy;
    let entries: Vec<Entry> = q
        .reg
        .by_pair(CAKE, USDT)
        .into_iter()
        .filter(|e| e.ticker == "CAKE")
        .collect();

    let classic = direct_route::quote(
        &via(&q, &t, side),
        &names(&q),
        &cake(),
        qty,
        entries,
        &[],
        &|_: &Entry| true,
        U256::from(GAS),
    )
    .await
    .expect("классика считает");
    let graph = graph_route::quote(
        &via(&q, &t, side),
        &names(&q),
        &cake(),
        qty,
        U256::from(GAS),
    )
    .await
    .expect("граф считает");

    let best = pick::pick(&q, classic.clone(), graph.clone(), side, 18).await;
    // Покупка: лучший — дешевле за монету; граф видит оба прямых пула (+ мост), поэтому не дороже классики.
    let price = |x: &arb::core::types::Quote| x.avg_price;
    let eps = Decimal::new(1, 6);
    assert!(
        price(&best) <= price(&classic) + eps,
        "лучший не дороже классики"
    );
    assert!(
        price(&best) <= price(&graph) + eps,
        "pick выбрал не худшее из двух"
    );
    assert_eq!(best.fillable_qty, qty);
}
