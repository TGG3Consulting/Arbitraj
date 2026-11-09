//! Файл: tests/bsc_dex_bundle_tip_plan.rs
//! Что делает: чаевые канала в ПЛАНЕ котировки обмена только у пути одной транзакцией (Тигран 09.11: согласовано —
//! план совпадает с фактом; exec/trade.rs sends_tip — тот же выбор пути, что у сделки, им пользуется quote/mod.rs).
//! Маршруты — графовая котировка на пулах реальной записи CAKE 2026-10-29 (tests/route_common, блок 130521896), как
//! tests/bsc_dex_bundle_wire.rs; без сети, моков логики нет. Проверяется:
//! - мульти-роутерный маршрут формы «бандл» без исполнителя (exec/bundle_send.rs → send::dispatch_slice, чаевых нет) —
//!   sends_tip = нет, хотя чаевые канала в настройках есть (send::planned_tip — Some);
//! - тот же маршрут при исполнителе кошелька (одна транзакция, exec/executor/trade.rs → txs::send с чаевыми) — да;
//! - прямая форма графа, режим Direct, маршрута нет, графовый маршрут неисполним (сделки нет) — да, как было.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{exec::trade, quote::graph_route, send}, tests/route_common.

mod route_common;

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{
    Asset, DexRoute, PoolKind, PoolRef, RouteHop, RouteLeg, RouteMode, Side, TokenTaxes,
};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::exec::trade;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::quote::Quoting;
use arb::venues::bsc_dex::quote::graph_route::{self, GraphForm};
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::Via;
use arb::venues::bsc_dex::send;

use route_common::{CAKE, CAKE_USDT_PCS, USDT};

/// Записанный адрес исполнителя из решения 132 (tests/core_wallet_mapping.rs).
const EXECUTOR: &str = "0x9ef558a5d437718dde44e00990e0efb9976cf794";

async fn set(p: &Parts, scope: Scope, n: &str, v: serde_json::Value) {
    p.ctx.settings.set(n, scope, Some(v), "тест").await.unwrap();
}

/// Графовая котировка 50k CAKE (сплит на несколько долей/роутеров из записи) — как tests/bsc_dex_bundle_wire.rs.
async fn graph_quote(q: &Quoting) -> arb::core::types::Quote {
    let t = TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    };
    let via = Via {
        q,
        coin: CAKE,
        usdt: USDT,
        side: Side::Buy,
        taxes: &t,
        coin_dec: 18,
        usdt_dec: 18,
    };
    let names = Names {
        usdt: (USDT, "USDT"),
        coin: (CAKE, "CAKE"),
        mids: &q.mids,
    };
    let cake = Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    };
    let amount = Decimal::from(50_000u32);
    graph_route::quote(&via, &names, &cake, amount, U256::from(1_000_000_000u64))
        .await
        .expect("котировка графа")
}

/// Один готовый канал бандлом с чаевыми — единственный случай, когда чаевые платятся (send::tip).
async fn tip_channel(p: &Parts) {
    route_common::channel(p, "club48", "bundle").await;
    let ch = Scope::Channel("club48".into());
    set(p, ch.clone(), "channel.tip_enabled", json!(true)).await;
    set(p, ch.clone(), "channel.tip_bnb", json!("0.0001")).await;
    let a = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";
    set(p, ch, "channel.tip_address", json!(a)).await;
}

#[tokio::test]
async fn bundle_route_plans_no_tip_single_tx_does() {
    let p = route_common::parts().await;
    tip_channel(&p).await;
    let v = Scope::Venue("bsc_dex".into());
    set(&p, v, "dex.bundle_max_routers", json!(5)).await;
    let q = p.quoting();
    assert!(send::planned_tip(&q.ctx).is_some(), "чаевые канала заданы");

    // Бандл свопов (без исполнителя) — чаевых не шлёт, в плане их нет.
    let r = graph_quote(&q).await;
    let route = r.route.as_ref().unwrap();
    assert!(route.executable, "{:?}", route.exec_note);
    assert_eq!(graph_route::graph_form(&q.ctx, route), GraphForm::Bundle);
    assert!(!trade::sends_tip(&q.ctx, Some(route), false));

    // Неисполнимый маршрут (сделки нет, путь не выбирается) — как было.
    let mut off = route.clone();
    off.executable = false;
    assert!(trade::sends_tip(&q.ctx, Some(&off), false));

    // Исполнитель кошелька — тот же маршрут одной транзакцией с чаевыми.
    route_common::executor(&p, EXECUTOR).await;
    let q = p.quoting();
    assert!(trade::sends_tip(&q.ctx, Some(route), false));
    // Нативная монета через исполнитель не идёт (executor::takes) — бандл, чаевых нет.
    assert!(!trade::sends_tip(&q.ctx, Some(route), true));
}

#[tokio::test]
async fn direct_and_no_route_keep_tip() {
    let p = route_common::parts().await;
    tip_channel(&p).await;
    let q = p.quoting();
    let leg = RouteLeg {
        share: Decimal::ONE,
        hops: vec![RouteHop {
            family: "pancakeswap_v3".into(),
            kind: PoolKind::V3,
            pool: PoolRef::Contract(CAKE_USDT_PCS),
        }],
        expected_out: Decimal::ONE,
    };
    let mut route = DexRoute {
        mode: RouteMode::Graph,
        legs: vec![leg],
        block: 1,
        gas_units: 0,
        executable: true,
        exec_note: None,
    };
    assert_eq!(graph_route::graph_form(&q.ctx, &route), GraphForm::Direct);
    assert!(trade::sends_tip(&q.ctx, Some(&route), false));
    route.mode = RouteMode::Direct;
    assert!(trade::sends_tip(&q.ctx, Some(&route), false));
    assert!(trade::sends_tip(&q.ctx, None, false));
}
