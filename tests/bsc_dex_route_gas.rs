//! Файл: tests/bsc_dex_route_gas.rs
//! Что делает: решение 122, часть R1 — учёт газа при выборе маршрута графа (dex.route_gas_aware) на пулах РЕАЛЬНОЙ
//! записи CAKE (tests/route_common, routes_2026-10-29; газ обмена — 120000 единиц на пул, как задаёт окружение записи):
//! газ пути в USDT = газ его пулов × цена газа, проданный в записанный пул WBNB — USDT (quote/rate.rs); не задано и
//! «нет» дают один и тот же маршрут (как до решения 122); при дорогом газе (2000 gwei) сплит с учётом газа открывает
//! меньше путей (на записи 1 против 3) и его чистая стоимость покупки (USDT + газ открытых путей) лучше, чем без учёта; без dex.native_wrapped
//! учёт газа не работает с понятной причиной.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::{graph_route, route_gas, rate, graph_legs, route_paths}, tests/route_common.

mod route_common;

use std::collections::{HashMap, HashSet};

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{Asset, Quote, Side, TokenTaxes};
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::num;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::graph;
use arb::venues::bsc_dex::quote::route_gas::GasUsdt;
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::Via;
use arb::venues::bsc_dex::quote::{graph_legs, graph_route, rate, route_paths};

use route_common::{BTCB, CAKE, USDT, WBNB};

/// 1 gwei — обычная цена газа BSC.
const GAS: u64 = 1_000_000_000;
/// Дорогой газ (2000 gwei): открытие пути стоит заметных USDT — сплит должен это учесть.
const GAS_HIGH: u64 = 2_000_000_000_000;

fn zero() -> TokenTaxes {
    TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    }
}

async fn set(p: &Parts, name: &str, v: serde_json::Value) {
    p.ctx
        .settings
        .set(name, Scope::Venue("bsc_dex".into()), Some(v), "тест")
        .await
        .unwrap();
}

/// Котировка графа: покупка `qty` CAKE при цене газа `gas` (wei).
async fn quote(p: &Parts, qty: u32, gas: u64) -> Quote {
    quote_side(p, Side::Buy, qty, gas).await
}

/// Котировка графа стороны `side` на `qty` CAKE при цене газа `gas` (wei).
async fn quote_side(p: &Parts, side: Side, qty: u32, gas: u64) -> Quote {
    let q = p.quoting();
    let t = zero();
    let v = Via {
        q: &q,
        coin: CAKE,
        usdt: USDT,
        side,
        taxes: &t,
        coin_dec: 18,
        usdt_dec: 18,
    };
    let names = Names {
        usdt: (USDT, "USDT"),
        coin: (CAKE, "CAKE"),
        mids: &p.mids,
    };
    let asset = Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    };
    graph_route::quote(&v, &names, &asset, Decimal::from(qty), U256::from(gas))
        .await
        .expect("котировка графа")
}

/// Газ всех открытых путей котировки в USDT (как считает учёт газа). Ошибок нет.
fn gas_of(p: &Parts, quote: &Quote, gas: u64) -> Decimal {
    let q = p.quoting();
    let r = quote.route.as_ref().unwrap();
    let units: u64 = r.legs.iter().map(|l| l.hops.len() as u64 * 120_000).sum();
    let wei = U256::from(gas) * U256::from(units);
    let got = rate::to_usdt(&q.ctx, &q.reg, USDT, WBNB, wei).expect("пул WBNB — USDT записи");
    num::to_decimal(got, 18).unwrap()
}

#[tokio::test]
async fn path_gas_is_sold_into_wbnb_pool() {
    let p = route_common::parts().await;
    set(&p, "dex.native_wrapped", json!(WBNB.to_string())).await;
    let q = p.quoting();
    let g = graph::build(&q.ctx, &q.reg, &q.tokens, 18).expect("граф");
    let allowed: HashSet<Address> = [WBNB, BTCB].into_iter().collect();
    let paths = route_paths::enumerate(&g, USDT, CAKE, &allowed, 3, 50);
    let two = paths
        .iter()
        .find(|p| p.hops() == 2)
        .expect("путь глубины 2");
    let dec: HashMap<Address, u8> = [CAKE, USDT, WBNB, BTCB]
        .into_iter()
        .map(|t| (t, 18))
        .collect();
    let legs = graph_legs::legs_for(&q, two, Side::Buy, 18, &dec).expect("ноги");
    let conv = GasUsdt::new(&q.ctx, &q.reg, (USDT, 18), U256::from(GAS)).expect("WBNB задан");
    let usdt = conv.path(&legs).expect("газ пути в USDT");
    // 2 пула × 120000 × 1 gwei = 0.00024 BNB, проданные в записанный пул WBNB — USDT.
    let want = rate::to_usdt(&q.ctx, &q.reg, USDT, WBNB, U256::from(240_000u64 * GAS)).unwrap();
    assert_eq!(usdt, num::to_decimal(want, 18).unwrap());
    assert!(
        usdt > Decimal::ZERO && usdt < Decimal::ONE,
        "газ двух обменов — центы: {usdt}"
    );
}

#[tokio::test]
async fn unset_and_off_are_the_same_route() {
    let p = route_common::parts().await;
    let a = quote(&p, 50_000, GAS_HIGH).await;
    set(&p, "dex.route_gas_aware", json!(false)).await;
    let b = quote(&p, 50_000, GAS_HIGH).await;
    let ra = a.route.unwrap();
    let rb = b.route.unwrap();
    assert_eq!(ra.legs.len(), rb.legs.len());
    assert_eq!(a.avg_price, b.avg_price);
}

#[tokio::test]
async fn gas_aware_split_not_worse_net() {
    let p = route_common::parts().await;
    set(&p, "dex.native_wrapped", json!(WBNB.to_string())).await;
    let plain = quote(&p, 50_000, GAS_HIGH).await;
    set(&p, "dex.route_gas_aware", json!(true)).await;
    let aware = quote(&p, 50_000, GAS_HIGH).await;
    assert_eq!(
        aware.fillable_qty, plain.fillable_qty,
        "покупка исполнена целиком"
    );
    let (lp, la) = (
        plain.route.as_ref().unwrap().legs.len(),
        aware.route.as_ref().unwrap().legs.len(),
    );
    // На записи: без учёта газа — 3 пути, с учётом — 1 (газ двух лишних путей дороже выигрыша сплита).
    assert!(la < lp, "с учётом газа путей меньше: {la} против {lp}");
    let net = |x: &Quote| x.avg_price * x.fillable_qty + gas_of(&p, x, GAS_HIGH);
    let (np, na) = (net(&plain), net(&aware));
    assert!(
        na < np,
        "чистая стоимость с учётом газа {na} лучше, чем без {np}"
    );
}

#[tokio::test]
async fn gas_aware_needs_wbnb() {
    let p = route_common::parts().await;
    set(&p, "dex.route_gas_aware", json!(true)).await;
    let q = p.quoting();
    let t = zero();
    let v = Via {
        q: &q,
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
        mids: &p.mids,
    };
    let asset = Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    };
    let e = graph_route::quote(&v, &names, &asset, Decimal::from(1000u32), U256::from(GAS))
        .await
        .expect_err("без dex.native_wrapped газ не перевести");
    assert_eq!(e.code, ErrorCode::NotConfigured);
    assert!(e.message.contains("dex.native_wrapped"), "{}", e.message);
}

/// Объём сделки фиксирован (продажа — все монеты; Тигран, 03.11): если новая доля не окупает газ, остаток идёт в
/// уже открытые доли, а когда они его не принимают — в новый путь, но не теряется. На записи продажа 1 000 000 CAKE
/// при газе 10^16 wei: прежнее правило исполняло лишь 700 000 (лучший путь кончился, новый «не окупал газ»); теперь
/// исполнено столько же, сколько без учёта газа. Покупка — так же.
#[tokio::test]
async fn gas_aware_fills_whole_volume() {
    let p = route_common::parts().await;
    set(&p, "dex.native_wrapped", json!(WBNB.to_string())).await;
    const GAS_EXTREME: u64 = 10_000_000_000_000_000;
    let mut plain = Vec::new();
    for side in [Side::Sell, Side::Buy] {
        for gas in [GAS_HIGH, GAS_EXTREME] {
            plain.push(quote_side(&p, side, 1_000_000, gas).await.fillable_qty);
        }
    }
    set(&p, "dex.route_gas_aware", json!(true)).await;
    let mut i = 0;
    for side in [Side::Sell, Side::Buy] {
        for gas in [GAS_HIGH, GAS_EXTREME] {
            let x = quote_side(&p, side, 1_000_000, gas).await;
            assert_eq!(
                x.fillable_qty, plain[i],
                "{side:?}, газ {gas}: весь объём распределён"
            );
            assert_eq!(
                x.fillable_qty,
                Decimal::from(1_000_000u32),
                "{side:?}, газ {gas}"
            );
            i += 1;
        }
    }
}
