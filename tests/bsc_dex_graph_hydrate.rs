//! Файл: tests/bsc_dex_graph_hydrate.rs
//! Что делает: решение 308, ревью №2 (D) — главный денежный инвариант И2 «граф без ожидания»: снимок графа даёт только
//! топологию, состояние каждого пула пути берётся СВЕЖИМ из реестра на момент котировки (route_cache::hydrate / fresh),
//! на РЕАЛЬНЫХ записях маршрута CAKE (tests/route_common: routes_2026-10-29 → 10-30; моков нет).
//!
//! 1. Промах кэша путей (dex.route_paths_ttl_ms не задана — каждый вызов промах): снимок опубликован по записям 10-29,
//!    затем пул WBNB–USDT получил запись 10-30 (снимок не менялся) — шаги путей через этот пул несут состояние 10-30 из
//!    реестра, а не копию 10-29 из снимка.
//! 2. Путь с пулом, ставшим недостоверным, спящим или удалённым после публикации снимка, отбрасывается; прочие пути
//!    остаются.
//! 3. Поиск циклов: циклы из снимка после route_cache::hydrate (так делает quote/cycle_scan.rs) — состояние из реестра,
//!    цикл с недостоверным пулом отброшен.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools, quote::route_cache, quote::route_paths}, tests/route_common.

mod route_common;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use serde_json::json;

use arb::core::types::PoolRef;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::graph_cache::graph_settings;
use arb::venues::bsc_dex::pools::graph_full;
use arb::venues::bsc_dex::pools::graph_snap::{Basis, Snapshot};
use arb::venues::bsc_dex::quote::route_cache::{self, Search};
use arb::venues::bsc_dex::quote::route_paths::{self, RoutePath};

use route_common::{BTCB, CAKE, CAKE_WBNB_PCS, USDT, WBNB, WBNB_USDT_V2};

/// Окружение 10-29 в режиме dex.graph_background = да со снимком, опубликованным по текущему реестру (без фоновой
/// задачи: поколение 0 — текущее у нового реестра).
async fn with_snapshot() -> Parts {
    let p = route_common::parts_from("tests/data/routes_2026-10-29").await;
    p.ctx
        .settings
        .set(
            "dex.graph_background",
            Scope::Venue("bsc_dex".into()),
            Some(json!(true)),
            "тест",
        )
        .await
        .unwrap();
    let bg = p.registry.graph_cache().bg();
    assert!(bg.start_once());
    let basis = Basis {
        settings: graph_settings(&p.ctx),
        usdt: USDT,
        usdt_dec: 18,
    };
    let (mut w, _) =
        graph_full::build(&p.ctx, &p.registry, &p.tokens, basis.clone(), &|| true).unwrap();
    let per_edge = p.ctx.int("dex.route_max_pools_per_edge").unwrap() as usize;
    let (g, _) = w.publish(per_edge);
    let snap = Snapshot {
        graph: Arc::new(g),
        basis,
        version: p.registry.version(),
        seq: bg.next_seq(),
        full: true,
        at: Instant::now(),
        at_utc: chrono::Utc::now(),
    };
    assert!(bg.publish(0, snap));
    p
}

/// Пути USDT → CAKE через промах кэша путей (промежуточные — WBNB, BTCB).
fn paths(p: &Parts) -> Vec<RoutePath> {
    let allowed: HashSet<_> = [WBNB, BTCB].into_iter().collect();
    let (max_hops, max_paths) = route_paths::limits(&p.ctx).unwrap();
    let s = Search {
        usdt: USDT,
        coin: CAKE,
        allowed: &allowed,
        max_hops,
        max_paths,
        cap: None,
    };
    route_cache::paths(&p.ctx, &p.registry, &p.tokens, 18, &s).unwrap()
}

/// Шаги через пул `r`: состояние шага равно состоянию записи реестра. Возвращает: сколько таких шагов.
fn steps_fresh(p: &Parts, ps: &[RoutePath], r: &PoolRef) -> usize {
    let now = p.registry.get(r).map(|e| e.pool.state);
    let mut n = 0;
    for st in ps.iter().flat_map(|x| x.steps.iter()) {
        if &st.edge.entry.pool.reference == r {
            assert_eq!(
                Some(st.edge.entry.pool.state.clone()),
                now,
                "шаг несёт состояние реестра"
            );
            n += 1;
        }
    }
    n
}

#[tokio::test]
async fn miss_path_takes_pool_state_from_registry() {
    let p = with_snapshot().await;
    let later = route_common::parts_from("tests/data/routes_2026-10-30").await;
    let r = PoolRef::Contract(WBNB_USDT_V2);
    let snap = p.registry.graph_cache().bg().load().unwrap();
    let old = snap
        .graph
        .between(WBNB, USDT)
        .iter()
        .find(|e| e.entry.pool.reference == r)
        .map(|e| e.entry.pool.state.clone());
    let old = old.expect("пул WBNB–USDT — ребро снимка");
    assert!(
        steps_fresh(&p, &paths(&p), &r) > 0,
        "есть путь через WBNB–USDT"
    );
    p.registry.put(later.registry.get(&r).unwrap());
    let new = p.registry.get(&r).unwrap().pool.state;
    assert_ne!(new, old, "записи 10-29 и 10-30 различаются");
    let ps = paths(&p);
    assert!(
        steps_fresh(&p, &ps, &r) > 0,
        "путь через WBNB–USDT — по состоянию реестра (10-30), не копии снимка"
    );
    assert!(
        Arc::ptr_eq(
            &snap.graph,
            &p.registry.graph_cache().bg().load().unwrap().graph
        ),
        "снимок не менялся"
    );
}

#[tokio::test]
async fn path_with_invalid_asleep_or_removed_pool_is_dropped() {
    let p = with_snapshot().await;
    let r = PoolRef::Contract(CAKE_WBNB_PCS);
    let uses = |ps: &[RoutePath]| {
        ps.iter()
            .any(|x| x.steps.iter().any(|s| s.edge.entry.pool.reference == r))
    };
    let all = paths(&p);
    assert!(uses(&all), "в снимке есть путь через CAKE–WBNB");
    let others = all
        .iter()
        .filter(|x| !x.steps.iter().any(|s| s.edge.entry.pool.reference == r))
        .count();
    p.registry.update(&r, |e| e.valid = false).unwrap();
    let ps = paths(&p);
    assert!(!uses(&ps), "недостоверный пул — путь отброшен");
    assert_eq!(ps.len(), others, "прочие пути остались");
    p.registry
        .update(&r, |e| {
            e.valid = true;
            e.asleep = true;
        })
        .unwrap();
    assert!(!uses(&paths(&p)), "спящий пул — путь отброшен");
    p.registry.retain(|e| e.pool.reference != r);
    assert!(!uses(&paths(&p)), "удалённый пул — путь отброшен");
}

#[tokio::test]
async fn cycles_are_hydrated_from_registry() {
    let p = with_snapshot().await;
    let later = route_common::parts_from("tests/data/routes_2026-10-30").await;
    let snap = p.registry.graph_cache().bg().load().unwrap();
    let cycles = route_paths::enumerate_cycles(&snap.graph, USDT, 3, 50);
    let r = PoolRef::Contract(WBNB_USDT_V2);
    p.registry.put(later.registry.get(&r).unwrap());
    let hyd = route_cache::hydrate(&p.registry, &cycles);
    assert!(
        steps_fresh(&p, &hyd, &r) > 0,
        "цикл через WBNB–USDT — по состоянию реестра"
    );
    let c = PoolRef::Contract(CAKE_WBNB_PCS);
    p.registry.update(&c, |e| e.valid = false).unwrap();
    let hyd = route_cache::hydrate(&p.registry, &cycles);
    assert!(
        !hyd.iter()
            .any(|x| x.steps.iter().any(|s| s.edge.entry.pool.reference == c)),
        "цикл с недостоверным пулом отброшен"
    );
}
