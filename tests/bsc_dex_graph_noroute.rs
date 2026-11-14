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
//! tests/route_common (реальная запись и окружение).

mod route_common;

use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::types::{
    Asset, CheckState, CoinCheck, Pool, PoolRef, PoolState, Quote, RouteMode, Side, TokenTaxes,
    VenueId,
};
use arb::gateway::errors::{ErrorCode, GatewayResult};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::Via;
use arb::venues::bsc_dex::quote::{graph_route, routing};
use route_common::USDT;

/// Синтетическая монета теста: её единственный пул — наш форк-пул (как в tests/bsc_dex_biswap_graph.rs).
const COIN: Address = address!("0x00000000000000000000000000000000000000c1");
/// Адрес пула теста.
const POOL: Address = address!("0x00000000000000000000000000000000000000c2");
/// Цена газа теста, wei.
const GAS: u64 = 2_000_000;

/// Нулевые налоги.
fn zero() -> TokenTaxes {
    TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    }
}

/// Монета теста.
fn coin() -> Asset {
    Asset {
        ticker: "COIN".into(),
        bsc_contract: Some(COIN),
    }
}

/// Настройка площадки bsc_dex (None — снять).
async fn venue(p: &Parts, name: &str, v: Option<Value>) {
    let s = Scope::Venue("bsc_dex".into());
    p.ctx.settings.set(name, s, v, "тест").await.unwrap();
}

/// Окружение записи с ОДНИМ пулом COIN-USDT семейства v2_fork (реальные резервы V2-пула записи), допущенным к
/// котировке; один бандловый канал и лимит роутеров бандла — форк-маршрут исполним (решения 84, 121 B3).
async fn scene() -> Parts {
    let p = route_common::parts().await;
    let (r0, r1) = p
        .registry
        .all()
        .into_iter()
        .find_map(|e| match e.pool.state {
            PoolState::V2 { reserve0, reserve1 } => Some((reserve0, reserve1)),
            _ => None,
        })
        .expect("V2-пул в записи routes_2026-10-29");
    let a = json!("0x0000000000000000000000000000000000000099");
    for (n, v) in [
        ("dex.kind", json!("v2_fork")),
        ("dex.pool_enabled", json!(true)),
        ("dex.factory", a.clone()),
        ("dex.router", a.clone()),
        ("dex.v2_fee_source", json!("fixed")),
        ("dex.v2_fee_pct", json!("0.25")),
    ] {
        let s = Scope::Dex("biswap".into());
        p.ctx.settings.set(n, s, Some(v), "тест").await.unwrap();
    }
    route_common::channel(&p, "club48", "bundle").await;
    venue(&p, "dex.bundle_max_routers", Some(json!(3))).await;
    p.tokens.set(&[
        Asset {
            ticker: "USDT".into(),
            bsc_contract: Some(USDT),
        },
        coin(),
    ]);
    p.tokens.remember_decimals(USDT, 18);
    p.tokens.remember_decimals(COIN, 18);
    p.checks.set_coin(CoinCheck {
        venue: VenueId("bsc_dex".into()),
        asset: coin(),
        taxes: CheckState::Known(zero()),
        source: "тест".into(),
        checked_at: Some(Utc::now()),
        wait: None,
    });
    let f = families::make("biswap", "v2_fork").unwrap();
    let (t0, t1) = if COIN < USDT {
        (COIN, USDT)
    } else {
        (USDT, COIN)
    };
    p.registry.put(Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(POOL),
            token0: t0,
            token1: t1,
            fee_pips: Some(2500),
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::V2 {
                reserve0: r0,
                reserve1: r1,
            },
            block: 1,
            received_at: Utc::now(),
        },
        ticker: "COIN".into(),
        coin: COIN,
        usdt: USDT,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
        asleep: false,
    });
    p.checks.update_pool(&PoolRef::Contract(POOL), |f| {
        f.gas_buy = Some(120_000);
        f.gas_sell = Some(120_000);
        f.quoter.buy = Some(Ok(()));
        f.quoter.sell = Some(Ok(()));
        f.taxes = Some(route_common::clean_taxes());
    });
    p
}

/// Выбор маршрута как в Quoting::priced (quote/routing.rs): пулы монеты с USDT из реестра, активные промежуточные.
async fn routed(p: &Parts, side: Side, smart: bool) -> GatewayResult<Quote> {
    let q = p.quoting();
    let mids = q
        .mids
        .active(&q.ctx)
        .into_iter()
        .map(|m| (m, 18u8))
        .collect();
    let input = routing::Input {
        asset: coin(),
        side,
        qty: Decimal::from(1000u32),
        coin: COIN,
        usdt: USDT,
        taxes: zero(),
        coin_dec: 18,
        usdt_dec: 18,
        entries: q.reg.by_pair(COIN, USDT),
        mids,
        gas: U256::from(GAS),
        usdt_ticker: "USDT".into(),
        smart,
    };
    routing::run(q, input).await
}

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
