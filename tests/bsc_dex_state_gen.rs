//! Файл: tests/bsc_dex_state_gen.rs
//! Что делает: решение 301 (е2) — поколение состояния записи реестра (pools/state_gen.rs) на РЕАЛЬНОЙ записи маршрута
//! CAKE (tests/route_common, routes_2026-10-29 и 10-30, шесть пулов V2 + Pancake/Uniswap V3):
//! - повторная запись того же состояния (перечитан на следующем блоке: новый synced_block, received_at) не меняет
//!   поколение, версия реестра растёт, глубина не пересчитывается (Book::calcs), граф после такой записи — тот же по
//!   содержанию;
//! - граф и глубины с памятью по поколению (живой реестр после перечитываний) равны графу и глубинам, посчитанным
//!   заново без памяти (новый реестр с теми же записями): пара → отсортированный список (пул, глубина);
//! - смена состояния (резервы пары V2 по записи 10-30) — новое поколение и пересчёт;
//! - решение 301 (е3): update пула, которого нет в реестре, версию реестра не меняет.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::{graph, registry, depth_book}, tests/route_common.

mod route_common;

use std::collections::BTreeMap;

use alloy_primitives::Address;

use arb::core::types::PoolRef;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::graph::{self, Graph};
use arb::venues::bsc_dex::pools::registry::Registry;

use route_common::{BTCB, CAKE, USDT, WBNB, WBNB_USDT_V2};

/// Пара → отсортированный список (пул, глубина текстом) по рёбрам графа; глубины — из книги реестра.
fn picture(g: &Graph, reg: &Registry) -> BTreeMap<(Address, Address), Vec<String>> {
    let toks = [CAKE, USDT, BTCB, WBNB];
    let mut out = BTreeMap::new();
    for (i, x) in toks.iter().enumerate() {
        for y in &toks[i + 1..] {
            let mut v: Vec<String> = g
                .between(*x, *y)
                .iter()
                .map(|e| {
                    let r = &e.entry.pool.reference;
                    format!("{r:?} {:?}", reg.depth().last(r))
                })
                .collect();
            v.sort();
            if !v.is_empty() {
                out.insert((*x, *y), v);
            }
        }
    }
    out
}

/// Новый реестр с теми же записями (без памяти глубины и графа).
fn fresh_copy(p: &Parts) -> Registry {
    let r = Registry::default();
    for e in p.registry.all() {
        r.put(e);
    }
    r
}

#[tokio::test]
async fn same_state_reread_keeps_generation_and_depth() {
    for dir in [
        "tests/data/routes_2026-10-29",
        "tests/data/routes_2026-10-30",
    ] {
        let p = route_common::parts_from(dir).await;
        let reg = &p.registry;
        let g1 = graph::build(&p.ctx, reg, &p.tokens, 18).unwrap();
        assert!(!g1.is_empty());
        let calcs = reg.depth().calcs();
        let gens: Vec<(PoolRef, u64)> = reg
            .all()
            .iter()
            .map(|e| (e.pool.reference.clone(), e.gen))
            .collect();
        assert!(gens.iter().all(|(_, g)| *g != 0), "у записей реестра поколение есть");
        // Каждый пул «перечитан» на следующем блоке с тем же состоянием (put и update).
        for e in reg.all() {
            let mut n = e.clone();
            n.synced_block += 1;
            n.synced_log = None;
            n.pool.block += 1;
            n.pool.received_at = chrono::Utc::now();
            reg.put(n);
            reg.update(&e.pool.reference, |x| x.synced_log = Some(7));
        }
        for (r, g) in &gens {
            assert_eq!(reg.get(r).unwrap().gen, *g, "то же состояние — то же поколение");
        }
        let g2 = graph::build(&p.ctx, reg, &p.tokens, 18).unwrap();
        assert_eq!(reg.depth().calcs(), calcs, "глубина не пересчитана ни у одного пула");
        assert_eq!(picture(&g1, reg), picture(&g2, reg));
        // Без памяти: новый реестр с теми же записями считает всё заново — та же картина.
        let f = fresh_copy(&p);
        let g3 = graph::build(&p.ctx, &f, &p.tokens, 18).unwrap();
        assert_eq!(picture(&g2, reg), picture(&g3, &f), "{dir}: память по поколению = расчёт заново");
    }
}

#[tokio::test]
async fn state_change_gives_new_generation() {
    let p = route_common::parts_from("tests/data/routes_2026-10-29").await;
    let later = route_common::parts_from("tests/data/routes_2026-10-30").await;
    let r = PoolRef::Contract(WBNB_USDT_V2);
    let before = p.registry.get(&r).unwrap();
    graph::build(&p.ctx, &p.registry, &p.tokens, 18).unwrap();
    let calcs = p.registry.depth().calcs();
    // Состояние пары V2 по записи следующего дня (другие резервы).
    let mut n = before.clone();
    n.pool.state = later.registry.get(&r).unwrap().pool.state;
    assert_ne!(n.pool.state, before.pool.state, "резервы в записях разные");
    p.registry.put(n);
    let after = p.registry.get(&r).unwrap();
    assert_ne!(after.gen, before.gen, "смена состояния — новое поколение");
    let g = graph::build(&p.ctx, &p.registry, &p.tokens, 18).unwrap();
    assert!(p.registry.depth().calcs() > calcs, "глубина пересчитана");
    let f = fresh_copy(&p);
    let g_fresh = graph::build(&p.ctx, &f, &p.tokens, 18).unwrap();
    assert_eq!(picture(&g, &p.registry), picture(&g_fresh, &f));
}

#[tokio::test]
async fn update_of_missing_pool_keeps_version() {
    let p = route_common::parts().await;
    let v = p.registry.version();
    let missing = PoolRef::Contract(Address::repeat_byte(0x11));
    assert!(p.registry.update(&missing, |e| e.valid = false).is_none());
    assert_eq!(p.registry.version(), v, "пула нет — версия прежняя");
    p.registry
        .update(&PoolRef::Contract(WBNB_USDT_V2), |e| e.synced_log = Some(3))
        .unwrap();
    assert_eq!(p.registry.version(), v + 1, "запись существующего пула — версия растёт");
}
