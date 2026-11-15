//! Файл: tests/bsc_dex_multi_pair_golden_routes.rs
//! Что делает: эталонный снимок поведения ПАРНЫХ пулов (решение 295, шаг 0) на записях маршрута CAKE routes_2026-10-29
//! (блок 130521896) и routes_2026-10-30 (блок 130556914): записи реестра (ссылка, пара, состояние), рёбра графа
//! (pools/graph.rs), пути route_paths::enumerate и route_best::enumerate, циклы route_paths::enumerate_cycles,
//! котировки pool_quote::swap по каждому пулу в обе стороны. Снимок записан кодом ДО правки многомонетных пулов;
//! после каждого шага правки текст обязан совпасть байт в байт (tests/golden_common).
//! Кто вызывает: cargo test --test bsc_dex_multi_pair_golden_routes.
//! От чего зависит: tests/route_common (шесть пулов записи), tests/golden_common, arb::venues::bsc_dex::{pools, quote}.

mod golden_common;
mod route_common;

use std::collections::HashSet;

use alloy_primitives::U256;

use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::graph;
use arb::venues::bsc_dex::quote::route_paths::RoutePath;
use arb::venues::bsc_dex::quote::{pool_quote, route_best, route_paths};

use golden_common::{check, sorted};
use route_common::{BTCB, CAKE, USDT, WBNB};

/// Строка пути: шаги «ссылка:вход>выход» через « ; ». Ошибок нет.
fn path_line(p: &RoutePath) -> String {
    p.steps
        .iter()
        .map(|s| {
            format!(
                "{:?}:{:#x}>{:#x}",
                s.edge.entry.pool.reference, s.token_in, s.token_out
            )
        })
        .collect::<Vec<_>>()
        .join(" ; ")
}

/// Записи реестра: ссылка, вид, пара записи, ориентация бота, состояние, число тиков. Ошибок нет.
fn registry(p: &Parts) -> String {
    sorted(
        p.registry
            .snapshot()
            .iter()
            .map(|e| {
                format!(
                    "{:?} fam={} t0={:#x} t1={:#x} coin={:#x} usdt={:#x} fee={:?} valid={} state={:?} ticks={:?}",
                    e.pool.reference,
                    e.family.id,
                    e.pool.token0,
                    e.pool.token1,
                    e.coin,
                    e.usdt,
                    e.pool.fee_pips,
                    e.valid,
                    e.pool.state,
                    e.ticks.as_ref().map(|t| t.ticks.len())
                )
            })
            .collect(),
    )
}

/// Котировки обмена каждым пулом в обе стороны: точный вход 1e15/1e18/1e20 и точный выход 1e18. Ошибок нет.
fn swaps(p: &Parts) -> String {
    let mut out = Vec::new();
    for e in p.registry.snapshot() {
        for tin in [e.pool.token0, e.pool.token1] {
            for (exact_in, amt) in [(true, 15u32), (true, 18), (true, 20), (false, 18)] {
                let r = pool_quote::swap(&p.ctx, &e, tin, exact_in, U256::from(10u8).pow(U256::from(amt)))
                    .map_err(|x| x.message);
                out.push(format!(
                    "{:?} in={tin:#x} exact_in={exact_in} 1e{amt} -> {r:?}",
                    e.pool.reference
                ));
            }
        }
    }
    sorted(out)
}

/// Полный снимок одной записи маршрута. Принимает: окружение записи. Возвращает: текст. Ошибок нет.
fn snapshot(p: &Parts) -> String {
    let g = graph::build(&p.ctx, &p.registry, &p.tokens, 18).expect("граф строится");
    let mut text = String::from("## registry\n");
    text += &registry(p);
    text += "## graph\n";
    let mut edges = Vec::new();
    for t in [USDT, CAKE, WBNB, BTCB] {
        for e in g.edges_from(t) {
            edges.push(format!("{t:#x} -> {:#x} via {:?}", e.other(t), e.entry.pool.reference));
        }
    }
    text += &sorted(edges);
    let allowed = HashSet::from([WBNB, BTCB]);
    for (name, a) in [("allowed", allowed.clone()), ("direct", HashSet::new())] {
        for (from, to) in [(USDT, CAKE), (CAKE, USDT)] {
            text += &format!("## paths {name} {from:#x}->{to:#x}\n");
            text += &sorted(route_paths::enumerate(&g, from, to, &a, 3, 500).iter().map(path_line).collect());
            text += &format!("## best {name} {from:#x}->{to:#x}\n");
            text += &sorted(route_best::enumerate(&g, from, to, &a, 3, 500).iter().map(path_line).collect());
        }
    }
    for base in [USDT, CAKE, WBNB, BTCB] {
        text += &format!("## cycles {base:#x}\n");
        text += &sorted(route_paths::enumerate_cycles(&g, base, 4, 500).iter().map(path_line).collect());
    }
    text += "## swaps\n";
    text += &swaps(p);
    text
}

#[tokio::test]
async fn paired_routes_unchanged_2026_10_29() {
    let p = route_common::parts_from("tests/data/routes_2026-10-29").await;
    check("routes_2026-10-29", &snapshot(&p));
}

#[tokio::test]
async fn paired_routes_unchanged_2026_10_30() {
    let p = route_common::parts_from("tests/data/routes_2026-10-30").await;
    check("routes_2026-10-30", &snapshot(&p));
}
