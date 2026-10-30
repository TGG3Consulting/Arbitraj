//! Файл: tests/bsc_dex_bundle_wire.rs
//! Что делает: флип executable и классификация формы графового маршрута (quote/graph_route.rs, решение 84, фаза
//! 80.8c-ii) на пулах реальной записи CAKE (tests/route_common), без реальной сети. Проверяет: мульти-роутерный
//! маршрут исполним (executable=true) только когда задан dex.bundle_max_routers и роутеров не больше него; не задан
//! или роутеров больше предела → не исполним (сигнал); graph_form различает прямую / один-UR / бандл (боевой путь
//! exec/trade.rs по ней выбирает прямой/умный/бандл). Отправку бандла сетью не гоняем.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::{graph_route, via, steps, Quoting}, tests/route_common.

mod route_common;

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{
    Asset, DexRoute, PoolKind, PoolRef, RouteHop, RouteLeg, RouteMode, Side, TokenTaxes,
};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::quote::Quoting;
use arb::venues::bsc_dex::quote::graph_route::{self, GraphForm};
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::Via;

use route_common::{CAKE, CAKE_USDT_PCS, CAKE_USDT_UNI, USDT};

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

fn via<'a>(q: &'a Quoting, t: &'a TokenTaxes) -> Via<'a> {
    Via {
        q,
        coin: CAKE,
        usdt: USDT,
        side: Side::Buy,
        taxes: t,
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

async fn set(p: &arb::venues::bsc_dex::parts::Parts, scope: Scope, n: &str, v: serde_json::Value) {
    p.ctx.settings.set(n, scope, Some(v), "тест").await.unwrap();
}

/// Котировка графа на 50k CAKE (сплит на несколько долей/роутеров из записи).
async fn quote(q: &Quoting) -> arb::core::types::Quote {
    let t = zero();
    graph_route::quote(
        &via(q, &t),
        &names(q),
        &cake(),
        Decimal::from(50_000u32),
        U256::from(GAS),
    )
    .await
    .expect("котировка графа")
}

#[tokio::test]
async fn multi_router_executable_only_with_bundle_max_routers() {
    // Без dex.bundle_max_routers мульти-роутерный (здесь — многодолевой) маршрут не исполним — сигнал.
    let p = route_common::parts().await;
    let q = p.quoting();
    let r = quote(&q).await;
    let route = r.route.as_ref().unwrap();
    assert!(route.legs.len() >= 2, "сплит на несколько долей");
    assert!(
        !route.executable,
        "без dex.bundle_max_routers бандл не исполним: {:?}",
        route.exec_note
    );
    assert!(
        route
            .exec_note
            .as_deref()
            .unwrap_or("")
            .contains("bundle_max_routers")
    );

    // Задали предел — маршрут собирается в бандл и исполним; форма — Bundle.
    let v = Scope::Venue("bsc_dex".into());
    set(&p, v, "dex.bundle_max_routers", json!(5)).await;
    let r = quote(&q).await;
    let route = r.route.as_ref().unwrap();
    assert!(route.executable, "с dex.bundle_max_routers бандл исполним");
    assert_eq!(graph_route::graph_form(&q.ctx, route), GraphForm::Bundle);
}

/// Сколько различных Universal Router'ов задействует маршрут (как считает collectable: по dex.universal_router вида
/// каждого хопа). Ошибок нет (все виды маршрута подключены в окружении).
fn distinct_routers(ctx: &arb::venues::bsc_dex::ctx::Ctx, route: &DexRoute) -> usize {
    let mut rs: Vec<alloy_primitives::Address> = Vec::new();
    for l in &route.legs {
        for h in &l.hops {
            let r = ctx.dex_addr("dex.universal_router", &h.family).unwrap();
            if !rs.contains(&r) {
                rs.push(r);
            }
        }
    }
    rs.len()
}

#[tokio::test]
async fn router_count_capped_by_bundle_max_routers() {
    // Uniswap — на отдельном Universal Router: прямая доля CAKE-USDT через Uniswap даёт второй роутер в маршруте.
    let p = route_common::parts().await;
    let d = Scope::Dex("uniswap_v3".into());
    let uni = json!("0x00000000000000000000000000000000000000bb");
    set(&p, d.clone(), "dex.universal_router", uni.clone()).await;
    set(&p, d, "dex.permit2", uni).await;
    let v = Scope::Venue("bsc_dex".into());
    let q = p.quoting();

    // Сколько роутеров реально в сплите 50k по этой записи — считаем из самого маршрута (сплит от предела не зависит).
    set(&p, v.clone(), "dex.bundle_max_routers", json!(9)).await;
    let route = quote(&q).await.route.unwrap();
    let n = distinct_routers(&q.ctx, &route);
    assert!(
        n >= 2,
        "по записи сплит 50k задействует оба прямых пула (Pancake и Uniswap) — 2 роутера, получили {n}"
    );

    // Предел на один меньше, чем в маршруте — бандлом не собрать (сигнал).
    set(&p, v.clone(), "dex.bundle_max_routers", json!(n as u64 - 1)).await;
    let route = quote(&q).await.route.unwrap();
    assert!(
        !route.executable,
        "{n} роутеров больше предела {} — не исполним: {:?}",
        n - 1,
        route.exec_note
    );
    assert!(route.exec_note.as_deref().unwrap_or("").contains("Router"));

    // Предел ровно по маршруту — собирается в бандл, исполним; форма — Bundle.
    set(&p, v, "dex.bundle_max_routers", json!(n as u64)).await;
    let route = quote(&q).await.route.unwrap();
    assert!(route.executable, "{n} роутеров ≤ предела {n} — исполним");
    assert_eq!(graph_route::graph_form(&q.ctx, &route), GraphForm::Bundle);
}

fn hop(family: &str, pool: alloy_primitives::Address) -> RouteHop {
    RouteHop {
        family: family.into(),
        kind: PoolKind::V3,
        pool: PoolRef::Contract(pool),
    }
}

fn leg(hops: Vec<RouteHop>) -> RouteLeg {
    RouteLeg {
        share: Decimal::ONE,
        hops,
        expected_out: Decimal::from(1u32),
    }
}

fn route(legs: Vec<RouteLeg>) -> DexRoute {
    DexRoute {
        mode: RouteMode::Graph,
        legs,
        block: 1,
        gas_units: 0,
        executable: true,
        exec_note: None,
    }
}

#[tokio::test]
async fn graph_form_classifies_direct_one_ur_and_bundle() {
    let p = route_common::parts().await;
    let q = p.quoting();
    // Прямая форма: одна доля, один хоп.
    let direct = route(vec![leg(vec![hop("pancakeswap_v3", CAKE_USDT_PCS)])]);
    assert_eq!(graph_route::graph_form(&q.ctx, &direct), GraphForm::Direct);

    // Бандл: доли на разных экосистемах (Uniswap версии 2.0 — не умный путь PancakeSwap).
    let bundle = route(vec![
        leg(vec![hop("pancakeswap_v3", CAKE_USDT_PCS)]),
        leg(vec![hop("uniswap_v3", CAKE_USDT_UNI)]),
    ]);
    assert_eq!(graph_route::graph_form(&q.ctx, &bundle), GraphForm::Bundle);

    // Один Universal Router PancakeSwap (умный путь): smart.* совпадают с dex.* пула и версия pancakeswap_infinity.
    let v = Scope::Venue("bsc_dex".into());
    let a = json!("0x0000000000000000000000000000000000000099");
    set(&p, v.clone(), "smart.universal_router", a.clone()).await;
    set(&p, v, "smart.permit2", a).await;
    let q = p.quoting();
    let one_ur = route(vec![
        leg(vec![hop("pancakeswap_v3", CAKE_USDT_PCS)]),
        leg(vec![hop("pancakeswap_v3", CAKE_USDT_PCS)]),
    ]);
    assert_eq!(graph_route::graph_form(&q.ctx, &one_ur), GraphForm::OneUr);
}
