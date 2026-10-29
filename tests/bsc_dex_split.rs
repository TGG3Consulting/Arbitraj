//! Файл: tests/bsc_dex_split.rs
//! Что делает: проверяет котировку по графу (quote/graph_route.rs, решение 80, PR 80.4 — оценка пути и сплит,
//! бэклог §7.3) на РЕАЛЬНОЙ записи CAKE 2026-10-29: сплит объёма между пулами прямой пары CAKE-USDT
//! (Pancake V3 + Uniswap V3) не хуже лучшего одиночного пула; amountOutMin (ожидаемый выход доли) считается;
//! пути глубины 2 и 3 из записи оцениваются нашей математикой. Состояния пулов настоящие (tests/route_common).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::{graph_route, route_calc, route_paths, graph_legs, select, steps, via},
//! pools::graph, pools::registry.

mod route_common;

use std::collections::{HashMap, HashSet};

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use arb::core::types::{Asset, PoolRef, Side, TokenTaxes};
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::graph;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::route_calc::{self, Leg};
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::Via;
use arb::venues::bsc_dex::quote::{graph_legs, graph_route, route_paths, select};

use route_common::{BTCB, CAKE, CAKE_USDT_PCS, CAKE_USDT_UNI, USDT, WBNB};

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

fn names<'a>(p: &'a Parts) -> Names<'a> {
    Names {
        usdt: (USDT, "USDT"),
        coin: (CAKE, "CAKE"),
        mids: &p.mids,
    }
}

fn via<'a>(
    q: &'a arb::venues::bsc_dex::quote::Quoting,
    taxes: &'a TokenTaxes,
    side: Side,
) -> Via<'a> {
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

fn dec_map() -> HashMap<Address, u8> {
    [CAKE, USDT, WBNB, BTCB]
        .into_iter()
        .map(|t| (t, 18))
        .collect()
}

/// Стоимость покупки `qty` CAKE через один пул CAKE-USDT (USDT, которые отдадим). None — пул не исполняет объём целиком.
fn single_buy_usdt(
    q: &arb::venues::bsc_dex::quote::Quoting,
    addr: Address,
    qty: Decimal,
) -> Option<Decimal> {
    let e: Entry = q.reg.get(&PoolRef::Contract(addr))?;
    let usable = select::usable(q, &e, Side::Buy, 18).ok()?;
    let leg = Leg {
        entry: e,
        usable,
        token_in: USDT,
        token_out: CAKE,
        dec_in: 18,
        dec_out: 18,
    };
    route_calc::eval(&q.ctx, &[leg], Side::Buy, qty, Decimal::ONE)
        .ok()
        .map(|p| p.usdt)
}

#[tokio::test]
async fn small_buy_single_leg() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let t = zero();
    let quote = graph_route::quote(
        &via(&q, &t, Side::Buy),
        &names(&p),
        &cake(),
        Decimal::from(1000u32),
        U256::from(GAS),
    )
    .await
    .expect("котировка графа");
    assert_eq!(quote.fillable_qty, Decimal::from(1000u32));
    let r = quote.route.as_ref().unwrap();
    assert!(!r.legs.is_empty());
    // amountOutMin — ожидаемый выход каждой доли — посчитан.
    assert!(r.legs.iter().all(|l| l.expected_out > Decimal::ZERO));
    assert!(
        quote.avg_price > Decimal::ONE && quote.avg_price < Decimal::from(5u32),
        "{}",
        quote.avg_price
    );
}

#[tokio::test]
async fn large_buy_splits_not_worse_than_single() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let t = zero();
    let qty = Decimal::from(50_000u32);
    let quote = graph_route::quote(
        &via(&q, &t, Side::Buy),
        &names(&p),
        &cake(),
        qty,
        U256::from(GAS),
    )
    .await
    .expect("котировка графа");
    assert_eq!(quote.fillable_qty, qty, "объём исполнен целиком сплитом");
    let r = quote.route.as_ref().unwrap();
    // Сплит задействован: объём разложен на несколько долей (у записи — оба прямых пула + 2-хоп).
    assert!(r.legs.len() >= 2, "сплит: {} долей", r.legs.len());
    // Доли прямой пары CAKE-USDT присутствуют (одиночные ноги), и реальный многошаговый путь из записи
    // (через WBNB) тоже взял часть объёма — сплит использует и прямые пулы, и мост.
    assert!(r.legs.iter().any(|l| l.hops.len() == 1));
    assert!(
        r.legs.iter().any(|l| l.hops.len() >= 2),
        "многошаговая доля задействована"
    );
    let graph_usdt = quote.avg_price * quote.fillable_qty;
    // Не хуже лучшего одиночного пула на тот же объём: кто смог исполнить целиком — граф не дороже; если ни один пул
    // в одиночку не тянет объём, сплит строго лучше (граф исполнил, одиночные — нет).
    let singles: Vec<Decimal> = [CAKE_USDT_PCS, CAKE_USDT_UNI]
        .into_iter()
        .filter_map(|a| single_buy_usdt(&q, a, qty))
        .collect();
    if let Some(best) = singles.iter().copied().reduce(Decimal::min) {
        let eps = graph_usdt / Decimal::from(1_000_000u32);
        assert!(
            graph_usdt <= best + eps,
            "граф {graph_usdt} дороже одиночного {best}"
        );
    } else {
        assert_eq!(
            quote.fillable_qty, qty,
            "сплит исполнил объём, которого не тянет одиночный пул"
        );
    }
}

#[tokio::test]
async fn sell_quote_works() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let t = zero();
    let quote = graph_route::quote(
        &via(&q, &t, Side::Sell),
        &names(&p),
        &cake(),
        Decimal::from(1000u32),
        U256::from(GAS),
    )
    .await
    .expect("котировка графа (продажа)");
    assert!(quote.avg_price > Decimal::ZERO && quote.fillable_qty > Decimal::ZERO);
    assert!(
        quote
            .route
            .unwrap()
            .legs
            .iter()
            .all(|l| l.expected_out > Decimal::ZERO)
    );
}

#[tokio::test]
async fn two_and_three_hop_paths_evaluate() {
    // Пути глубины 2 (через WBNB) и 3 (через BTCB и WBNB) из записи оцениваются нашей математикой.
    let p = route_common::parts().await;
    let q = p.quoting();
    let g = graph::build(&q.ctx, &q.reg, &q.tokens, 18).expect("граф");
    let allowed: HashSet<Address> = [WBNB, BTCB].into_iter().collect();
    let paths = route_paths::enumerate(&g, USDT, CAKE, &allowed, 3, 50);
    let dec = dec_map();
    // Депт-2 (через WBNB) — реально исполнимый путь: оценивается нашей математикой, цена рядом с рынком.
    let two = paths
        .iter()
        .find(|p| p.hops() == 2)
        .expect("депт-2 путь из записи");
    let legs2 = graph_legs::legs_for(&q, two, Side::Buy, 18, &dec).expect("ноги депт-2 допущены");
    assert_eq!(legs2.len(), 2);
    let out2 = route_calc::eval(
        &q.ctx,
        &legs2,
        Side::Buy,
        Decimal::from(1000u32),
        Decimal::ONE,
    )
    .expect("депт-2 считается");
    assert_eq!(out2.coins, Decimal::from(1000u32));
    assert!(out2.usdt > Decimal::ZERO && out2.usdt / out2.coins < Decimal::from(5u32));

    // Депт-3 (через BTCB и WBNB) перечисляется и раскладывается на три допущенные ноги из реальной записи —
    // пайплайн глубины 3 работает. Экономически на этой записи путь неинтересен (узкое прочитанное окно пула
    // CAKE-BTCB: исполняет лишь крошечный объём и по запредельной цене), поэтому в сплит 50k он не попал.
    // Проверяем, что путь СТРОИТСЯ и ОЦЕНИВАЕТСЯ на малом объёме (даёт определённый результат).
    let three = paths
        .iter()
        .find(|p| p.hops() == 3)
        .expect("депт-3 путь из записи");
    let legs3 = graph_legs::legs_for(&q, three, Side::Buy, 18, &dec).expect("ноги депт-3 допущены");
    assert_eq!(legs3.len(), 3);
    let out3 = route_calc::eval(&q.ctx, &legs3, Side::Buy, Decimal::ONE, Decimal::ONE)
        .expect("депт-3 считается на малом объёме");
    assert!(out3.coins > Decimal::ZERO && out3.usdt > Decimal::ZERO);
}
