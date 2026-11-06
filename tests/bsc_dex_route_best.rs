//! Файл: tests/bsc_dex_route_best.rs
//! Что делает: решение 187 — отбор лучших путей графа (quote/route_best.rs) на пулах РЕАЛЬНОЙ записи CAKE 2026-10-29
//! (tests/route_common: CAKE-USDT ×2, CAKE-WBNB, WBNB-USDT, CAKE-BTCB, BTCB-WBNB; состояния настоящие, блок 130521896).
//! Проверяется: перебор идёт короткими путями первыми и предел перебора не теряет прямые пути; при заданной
//! dex.route_search_max_paths и пределе 2 пути покупка не дороже лучшего одиночного пути записи на тот же объём
//! (раньше в сплит шли первые 2 найденных пути — какие попадутся); продажа тоже работает.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::{graph_route, route_best, route_calc, graph_legs}, pools::graph,
//! tests/route_common.

mod route_common;

use std::collections::{HashMap, HashSet};

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{Asset, Quote, Side, TokenTaxes};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::graph;
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::Via;
use arb::venues::bsc_dex::quote::{graph_legs, graph_route, route_best, route_calc};

use route_common::{BTCB, CAKE, USDT, WBNB};

const GAS: u64 = 1_000_000_000;

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

async fn quote(p: &Parts, side: Side, qty: u32) -> Quote {
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
    graph_route::quote(&v, &names, &asset, Decimal::from(qty), U256::from(GAS))
        .await
        .expect("котировка графа")
}

/// USDT покупки `qty` CAKE каждым путём записи в одиночку (кто тянет объём целиком). Ошибок нет.
fn singles(p: &Parts, qty: Decimal) -> Vec<Decimal> {
    let q = p.quoting();
    let g = graph::build(&q.ctx, &q.reg, &q.tokens, 18).expect("граф");
    let allowed: HashSet<Address> = [WBNB, BTCB].into_iter().collect();
    let dec: HashMap<Address, u8> = [CAKE, USDT, WBNB, BTCB]
        .into_iter()
        .map(|t| (t, 18))
        .collect();
    route_best::enumerate(&g, USDT, CAKE, &allowed, 3, 100)
        .iter()
        .filter_map(|path| {
            let legs = graph_legs::legs_for(&q, path, Side::Buy, 18, &dec).ok()?;
            route_calc::eval(&q.ctx, &legs, Side::Buy, qty, Decimal::ONE)
                .ok()
                .map(|r| r.usdt)
        })
        .collect()
}

#[tokio::test]
async fn short_paths_first_and_cap_keeps_direct() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let g = graph::build(&q.ctx, &q.reg, &q.tokens, 18).expect("граф");
    let allowed: HashSet<Address> = [WBNB, BTCB].into_iter().collect();
    let all = route_best::enumerate(&g, USDT, CAKE, &allowed, 3, 100);
    let hops: Vec<usize> = all.iter().map(|x| x.hops()).collect();
    assert_eq!(hops, vec![1, 1, 2, 3], "короткие первыми: {hops:?}");
    // Предел 2 — оба прямых пути, длинные не вытесняют короткие.
    let two = route_best::enumerate(&g, USDT, CAKE, &allowed, 3, 2);
    assert!(two.iter().all(|x| x.hops() == 1));
    assert_eq!(two.len(), 2);
}

#[tokio::test]
async fn best_two_not_worse_than_best_single() {
    let p = route_common::parts().await;
    set(&p, "dex.route_search_max_paths", json!(100)).await;
    set(&p, "dex.route_max_paths", json!(2)).await;
    for qty in [1_000u32, 20_000, 50_000] {
        let d = Decimal::from(qty);
        let got = quote(&p, Side::Buy, qty).await;
        assert_eq!(got.fillable_qty, d, "объём {qty} исполнен");
        let usdt = got.avg_price * got.fillable_qty;
        if let Some(best) = singles(&p, d).into_iter().reduce(Decimal::min) {
            let eps = usdt / Decimal::from(1_000_000u32);
            assert!(
                usdt <= best + eps,
                "{qty}: граф {usdt} дороже лучшего пути {best}"
            );
        }
        let n = got.route.as_ref().unwrap().legs.len();
        assert!(n <= 2, "не больше dex.route_max_paths путей: {n}");
    }
}

#[tokio::test]
async fn sell_with_best_selection() {
    let p = route_common::parts().await;
    set(&p, "dex.route_search_max_paths", json!(100)).await;
    set(&p, "dex.route_max_paths", json!(2)).await;
    let got = quote(&p, Side::Sell, 1_000).await;
    assert_eq!(got.fillable_qty, Decimal::from(1_000u32));
    assert!(got.avg_price > Decimal::ZERO);
}

/// Решение 189: набор путей из кэша (dex.route_paths_ttl_ms) даёт ту же котировку, что граф и перебор на каждой
/// котировке; состояние пулов при этом свежее — пул, уснувший после записи набора в кэш, выпадает так же, как при
/// полном перестроении графа.
#[tokio::test]
async fn paths_cache_same_quote_and_fresh_state() {
    let p = route_common::parts().await;
    set(&p, "dex.route_search_max_paths", json!(100)).await;
    let base = quote(&p, Side::Buy, 20_000).await;
    set(&p, "dex.route_paths_ttl_ms", json!(60_000)).await;
    let miss = quote(&p, Side::Buy, 20_000).await;
    let hit = quote(&p, Side::Buy, 20_000).await;
    for q in [&miss, &hit] {
        assert_eq!(q.avg_price, base.avg_price);
        assert_eq!(q.fillable_qty, base.fillable_qty);
    }
    // Усыпить один прямой пул CAKE-USDT: кэш набора путей жив, но путь через спящий пул должен выпасть.
    let q = p.quoting();
    let sleepy = q
        .reg
        .all()
        .into_iter()
        .find(|e| e.coin == CAKE && e.usdt == USDT)
        .expect("прямой пул CAKE-USDT")
        .pool
        .reference;
    q.reg.update(&sleepy, |e| e.asleep = true);
    let cached = quote(&p, Side::Buy, 20_000).await;
    p.ctx
        .settings
        .set(
            "dex.route_paths_ttl_ms",
            Scope::Venue("bsc_dex".into()),
            None,
            "тест",
        )
        .await
        .unwrap();
    let rebuilt = quote(&p, Side::Buy, 20_000).await;
    assert_eq!(
        cached.avg_price, rebuilt.avg_price,
        "кэш набора путей ≠ полное перестроение"
    );
    assert_eq!(cached.fillable_qty, rebuilt.fillable_qty);
}
