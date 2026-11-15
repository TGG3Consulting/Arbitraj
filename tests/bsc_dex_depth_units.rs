//! Файл: tests/bsc_dex_depth_units.rs
//! Что делает: решение 307 (задача t137) — кривая пула для глубины (band_units) не пересчитывается при смене курса
//! встречного токена, на РЕАЛЬНЫХ записях маршрута CAKE (tests/route_common: routes_2026-10-29 и 10-30):
//! - пул CAKE–WBNB (встречный WBNB): пул WBNB–USDT получил состояние записи 10-30 (новое поколение) — глубина
//!   CAKE–WBNB пересчитана (курс другой), но кривая из памяти: счётчик расчётов кривой (Book::calcs) не вырос;
//! - глубина после этого равна глубине, посчитанной заново без памяти (новый реестр с теми же записями) — бит в бит;
//! - картина графа «пара → (пул, глубина)» равна расчёту заново.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::{graph, registry, depth_book}, tests/route_common.

mod route_common;

use arb::core::types::PoolRef;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::graph;
use arb::venues::bsc_dex::pools::registry::Registry;

use route_common::{CAKE_BTCB_UNI, CAKE_USDT_PCS, CAKE_WBNB_PCS, WBNB_USDT_V2};

/// Новый реестр с теми же записями (без памяти глубины).
fn fresh_copy(p: &Parts) -> Registry {
    let r = Registry::default();
    for e in p.registry.all() {
        r.put(e);
    }
    r
}

#[tokio::test]
async fn rate_change_keeps_curve() {
    let p = route_common::parts_from("tests/data/routes_2026-10-29").await;
    let later = route_common::parts_from("tests/data/routes_2026-10-30").await;
    let reg = &p.registry;
    graph::build(&p.ctx, reg, &p.tokens, 18).unwrap();
    let cake_wbnb = PoolRef::Contract(CAKE_WBNB_PCS);
    let before = reg.depth().last(&cake_wbnb).expect("глубина CAKE–WBNB посчитана");
    // Курс WBNB: пул WBNB–USDT — состояние записи следующего дня.
    let r = PoolRef::Contract(WBNB_USDT_V2);
    let mut n = reg.get(&r).unwrap();
    n.pool.state = later.registry.get(&r).unwrap().pool.state;
    reg.put(n);
    let calcs = reg.depth().calcs();
    let e = reg.get(&cake_wbnb).unwrap();
    let d = reg.depth().depth(&p.ctx, reg, &p.tokens, &e, 18).unwrap();
    assert_eq!(reg.depth().calcs(), calcs, "кривая CAKE–WBNB из памяти — сменился только курс");
    assert_ne!(d.band, before.band, "курс другой — глубина в USDT другая");
    // Без памяти — то же число.
    let f = fresh_copy(&p);
    let fe = f.get(&cake_wbnb).unwrap();
    let fd = f.depth().depth(&p.ctx, &f, &p.tokens, &fe, 18).unwrap();
    assert_eq!(d.band, fd.band, "память кривой = расчёт заново");
    // Курс CAKE (встречный пула CAKE–BTCB): пул CAKE–USDT Pancake — состояние 10-30.
    let r2 = PoolRef::Contract(CAKE_USDT_PCS);
    let mut n2 = reg.get(&r2).unwrap();
    n2.pool.state = later.registry.get(&r2).unwrap().pool.state;
    reg.put(n2);
    let g = graph::build(&p.ctx, reg, &p.tokens, 18).unwrap();
    let f = fresh_copy(&p);
    let gf = graph::build(&p.ctx, &f, &p.tokens, 18).unwrap();
    let cake_btcb = PoolRef::Contract(CAKE_BTCB_UNI);
    assert_eq!(
        reg.depth().last(&cake_btcb).map(|x| x.band),
        f.depth().last(&cake_btcb).map(|x| x.band),
        "CAKE–BTCB: глубина по новому курсу CAKE равна расчёту заново"
    );
    assert_eq!(g.len(), gf.len(), "граф — те же рёбра");
}
