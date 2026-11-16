//! Файл: tests/bsc_dex_legs_prep_311.rs
//! Что делает: решение 311 (А) — одна подготовка глубины (pools/depth_key.rs Prep) на котировку графа даёт те же ноги,
//! что подготовка на каждый хоп (как до решения): на пулах РЕАЛЬНОЙ записи CAKE 2026-10-29 (tests/route_common:
//! CAKE-USDT ×2, CAKE-WBNB, WBNB-USDT, CAKE-BTCB, BTCB-WBNB; у CAKE-WBNB и BTCB-WBNB встречный токен WBNB — глубина
//! переводится в USDT через пул WBNB-USDT, Prep::rate). Для каждого пути графа (до 3 хопов) и обеих сторон:
//! допуск хопов (Usable), пулы, токены, decimals — те же; расчёт пути (route_calc::eval) на нескольких объёмах —
//! те же суммы.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools::{graph, depth_book}, quote::{graph_legs, route_best, route_calc}},
//! tests/route_common.

mod route_common;

use std::collections::{HashMap, HashSet};

use alloy_primitives::Address;
use rust_decimal::Decimal;

use arb::core::types::Side;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::pools::depth_book::Book;
use arb::venues::bsc_dex::pools::graph;
use arb::venues::bsc_dex::quote::route_calc::Leg;
use arb::venues::bsc_dex::quote::{graph_legs, route_best, route_calc};

use route_common::{BTCB, CAKE, USDT, WBNB};

/// Ноги совпадают поштучно: пул и его поколение, токены, decimals, допуск (газ, крючок, налог пула).
fn same_legs(a: &[Leg], b: &[Leg]) {
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b) {
        assert_eq!(x.entry.pool.reference, y.entry.pool.reference);
        assert_eq!(x.entry.state_gen, y.entry.state_gen);
        assert_eq!((x.token_in, x.token_out), (y.token_in, y.token_out));
        assert_eq!((x.dec_in, x.dec_out), (y.dec_in, y.dec_out));
        assert_eq!(x.usable.gas, y.usable.gas);
        assert_eq!(x.usable.hook_pct, y.usable.hook_pct);
        assert_eq!(x.usable.tax_pct, y.usable.tax_pct);
    }
}

/// Расчёт пути на объёмах 1, 10, 100, 1000: суммы обеих ног одинаковы (или одна и та же ошибка). Ошибок нет.
fn same_sums(ctx: &Ctx, a: &[Leg], b: &[Leg], side: Side) {
    for n in [1u32, 10, 100, 1_000] {
        let qty = Decimal::from(n);
        let x = route_calc::eval(ctx, a, side, qty, Decimal::ONE);
        let y = route_calc::eval(ctx, b, side, qty, Decimal::ONE);
        match (x, y) {
            (Ok(x), Ok(y)) => {
                assert_eq!(x.coins, y.coins, "монет, объём {n}");
                assert_eq!(x.usdt, y.usdt, "USDT, объём {n}");
            }
            (Err(x), Err(y)) => assert_eq!(x.message, y.message),
            _ => panic!("объём {n}: расчёт разошёлся"),
        }
    }
}

#[tokio::test]
async fn one_prep_same_legs_and_sums() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let g = graph::build(&q.ctx, &q.reg, &q.tokens, 18).expect("граф");
    let allowed: HashSet<Address> = [WBNB, BTCB].into_iter().collect();
    let dec: HashMap<Address, u8> = [CAKE, USDT, WBNB, BTCB]
        .into_iter()
        .map(|t| (t, 18))
        .collect();
    let paths = route_best::enumerate(&g, USDT, CAKE, &allowed, 3, 100);
    let long = paths.iter().any(|x| x.hops() > 1);
    assert!(long, "есть пути через WBNB/BTCB");
    let prep = Book::prep(&q.ctx, &q.tokens).expect("глубина");
    let mut checked = 0;
    for side in [Side::Buy, Side::Sell] {
        for path in &paths {
            let old = graph_legs::legs_for(&q, path, side, 18, &dec);
            let new = graph_legs::legs_prep(&q, path.clone(), side, 18, &dec, Some(&prep));
            match (&old, &new) {
                (Ok(a), Ok(b)) => {
                    same_legs(a, b);
                    same_sums(&q.ctx, a, b, side);
                    checked += 1;
                }
                (Err(a), Err(b)) => assert_eq!(a, b, "та же причина отказа"),
                _ => panic!("{side:?}: допуск пути разошёлся"),
            }
        }
    }
    assert!(checked > 0, "хотя бы один путь допущен");
}
