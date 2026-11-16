//! Файл: tests/bsc_dex_graph_snapshot.rs
//! Что делает: решение 308 (задача t137) — «граф без ожидания» на РЕАЛЬНЫХ записях маршрута CAKE (tests/route_common:
//! routes_2026-10-29 → routes_2026-10-30, шесть пулов V2 + Pancake/Uniswap V3; моков нет — каждое изменение реестра —
//! настоящая записанная запись пула).
//!
//! 1. Накопительная карта = сборка с нуля на КАЖДОЙ версии реестра (инвариант И6): пулы по одному получают записи
//!    следующего дня (включая WBNB–USDT — курс встречного WBNB у зависимых пулов), затем пул становится недостоверным,
//!    спящим и удаляется; после каждого шага круг (graph_round) по отмеченным реестром пулам и сверка с graph_full
//!    (graph_diff пуст), а набор пулов по парам равен старому graph::build.
//! 2. Самопроверка ловит «потерянное» изменение: запись WBNB–USDT следующего дня положена в реестр, а отметка изменения
//!    потеряна (набор очищен) — сверка находит отличия, доведённая карта равна сборке с нуля.
//! 3. Предохранитель: снимок опубликован, реестр изменился, отставание больше dex.graph_max_lag_ms — маршрута графа нет
//!    (StaleData с QUIET), счётчик предохранителя растёт; без предела — снимок отдаётся.
//! 4. Владелец целиком: подписка на рынок (Parts::pools) при dex.graph_background = да — фоновая задача публикует снимок
//!    сама, graph::cached отдаёт его без построения в расчёте; изменение пула — новый снимок той же картины, что build.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::{graph, graph_full, graph_round, graph_work, graph_diff, graph_snap,
//! graph_read, graph_cache, graph_stats}, tests/route_common.

mod route_common;

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy_primitives::Address;
use serde_json::json;

use arb::core::types::PoolRef;
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::depth_book::label as depth_label;
use arb::venues::bsc_dex::pools::graph::{self, Graph};
use arb::venues::bsc_dex::pools::graph_cache::graph_settings;
use arb::venues::bsc_dex::pools::graph_snap::{Basis, Snapshot};
use arb::venues::bsc_dex::pools::graph_work::Work;
use arb::venues::bsc_dex::pools::{graph_diff, graph_full, graph_read, graph_round, graph_stats};

use route_common::{
    BTCB_WBNB_PCS, CAKE_BTCB_UNI, CAKE_USDT_PCS, CAKE_USDT_UNI, CAKE_WBNB_PCS, USDT, WBNB_USDT_V2,
};

/// Все шесть пулов записи.
const POOLS: [Address; 6] = [
    WBNB_USDT_V2,
    CAKE_USDT_PCS,
    CAKE_WBNB_PCS,
    CAKE_USDT_UNI,
    CAKE_BTCB_UNI,
    BTCB_WBNB_PCS,
];

/// dex.route_max_pools_per_edge записи. Ошибок нет (паника теста — настройки нет).
fn per_edge(p: &Parts) -> usize {
    p.ctx.int("dex.route_max_pools_per_edge").unwrap() as usize
}

fn basis(p: &Parts) -> Basis {
    Basis {
        settings: graph_settings(&p.ctx),
        usdt: USDT,
        usdt_dec: 18,
    }
}

/// Карта с нуля по текущему реестру, топ собран.
fn full(p: &Parts) -> Work {
    let (mut w, _) = graph_full::build(&p.ctx, &p.registry, &p.tokens, basis(p), &|| true).unwrap();
    w.publish(per_edge(p));
    w
}

/// Пара → отсортированные пулы по рёбрам графа.
fn sets(g: &Graph) -> BTreeMap<(Address, Address), Vec<String>> {
    let toks = [
        route_common::CAKE,
        USDT,
        route_common::BTCB,
        route_common::WBNB,
    ];
    let mut out = BTreeMap::new();
    for (i, x) in toks.iter().enumerate() {
        for y in &toks[i + 1..] {
            let mut v: Vec<String> = g
                .between(*x, *y)
                .iter()
                .map(|e| depth_label(&e.entry.pool.reference))
                .collect();
            v.sort();
            if !v.is_empty() {
                out.insert((*x, *y), v);
            }
        }
    }
    out
}

/// Круг по отмеченным реестром пулам и сверка с картой с нуля и с graph::build. Принимает: окружение, карту, шаг.
fn step_and_check(p: &Parts, inc: &mut Work, what: &str) {
    let (refs, _) = p.registry.graph_cache().bg().changes.drain();
    assert!(!refs.is_empty(), "{what}: реестр отметил изменение");
    let out = graph_round::round(
        &p.ctx,
        &p.registry,
        &p.tokens,
        inc,
        refs,
        per_edge(p),
        &|| true,
    )
    .unwrap();
    let f = full(p);
    let d = graph_diff::diff(&inc.picture(), &f.picture());
    assert!(d.is_empty(), "{what}: накопительная ≠ с нуля: {d:?}");
    let old = graph::build(&p.ctx, &p.registry, &p.tokens, 18).unwrap();
    assert_eq!(
        sets(&out.graph),
        sets(&old),
        "{what}: пулы по парам = graph::build"
    );
}

#[tokio::test]
async fn incremental_equals_full_on_each_version() {
    let p = route_common::parts_from("tests/data/routes_2026-10-29").await;
    let later = route_common::parts_from("tests/data/routes_2026-10-30").await;
    let mut inc = full(&p);
    assert!(!inc.is_empty());
    p.registry.graph_cache().bg().changes.track(true);
    for a in POOLS {
        let r = PoolRef::Contract(a);
        p.registry.put(later.registry.get(&r).unwrap());
        step_and_check(&p, &mut inc, &format!("запись 10-30 пула {a}"));
    }
    let r = PoolRef::Contract(CAKE_USDT_UNI);
    p.registry.update(&r, |e| e.valid = false).unwrap();
    step_and_check(&p, &mut inc, "пул недостоверен");
    p.registry.update(&r, |e| e.valid = true).unwrap();
    step_and_check(&p, &mut inc, "пул снова достоверен");
    p.registry.update(&r, |e| e.asleep = true).unwrap();
    step_and_check(&p, &mut inc, "пул спит");
    p.registry.retain(|e| e.pool.reference != r);
    step_and_check(&p, &mut inc, "пул удалён");
    // Пул WBNB–USDT удалён: зависимые по курсу WBNB теряют глубину в USD — тоже без расхождений.
    let w = PoolRef::Contract(WBNB_USDT_V2);
    p.registry.retain(|e| e.pool.reference != w);
    step_and_check(&p, &mut inc, "пул курса WBNB удалён");
}

#[tokio::test]
async fn self_check_catches_lost_change() {
    let p = route_common::parts_from("tests/data/routes_2026-10-29").await;
    let later = route_common::parts_from("tests/data/routes_2026-10-30").await;
    let inc = full(&p);
    let bg = p.registry.graph_cache().bg();
    bg.changes.track(true);
    let r = PoolRef::Contract(WBNB_USDT_V2);
    p.registry.put(later.registry.get(&r).unwrap());
    // Отметка потеряна: накопительная карта о записи не узнала.
    let _ = bg.changes.drain();
    let (f, _) = graph_full::build(&p.ctx, &p.registry, &p.tokens, basis(&p), &|| true).unwrap();
    let rec = graph_full::reconcile(
        &p.ctx,
        &p.registry,
        &p.tokens,
        Some(inc),
        f,
        HashSet::new(),
        per_edge(&p),
        &|| true,
    )
    .map_err(|e| e.2.clone())
    .unwrap();
    let d = rec.diff.expect("было с чем сравнивать");
    assert!(!d.is_empty(), "потерянное изменение найдено сверкой");
    // Отличие — у пула курса или у пулов со встречным WBNB (их глубина в USD — по курсу WBNB).
    let touched =
        [WBNB_USDT_V2, CAKE_WBNB_PCS, BTCB_WBNB_PCS].map(|a| depth_label(&PoolRef::Contract(a)));
    assert!(
        d.iter().any(|l| touched.iter().any(|t| l.contains(t))),
        "{d:?}"
    );
    let again = full(&p);
    assert!(
        graph_diff::diff(&rec.work.picture(), &again.picture()).is_empty(),
        "после замены — как с нуля"
    );
}

#[tokio::test]
async fn fuse_drops_graph_route_when_snapshot_lags() {
    let p = route_common::parts_from("tests/data/routes_2026-10-29").await;
    let set = |n: &'static str, v| {
        let ctx = p.ctx.clone();
        async move {
            ctx.settings
                .set(n, Scope::Venue("bsc_dex".into()), Some(v), "тест")
                .await
                .unwrap();
        }
    };
    set("dex.graph_background", json!(true)).await;
    let bg = p.registry.graph_cache().bg();
    // Снимок из настоящей карты, без фоновой задачи (поколение 0 — текущее у нового реестра).
    assert!(bg.start_once());
    let mut w = full(&p);
    let (g, _) = w.publish(per_edge(&p));
    let snap = Snapshot {
        graph: Arc::new(g),
        basis: basis(&p),
        version: p.registry.version(),
        seq: bg.next_seq(),
        full: true,
        at: Instant::now(),
        at_utc: chrono::Utc::now(),
    };
    assert!(bg.publish(0, snap));
    let got = graph::cached(&p.ctx, &p.registry, &p.tokens, 18).unwrap();
    assert!(
        Arc::ptr_eq(&got, &bg.load().unwrap().graph),
        "расчёт берёт опубликованный снимок"
    );
    // Реестр изменился, снимок — нет.
    bg.changes.track(true);
    let r = PoolRef::Contract(CAKE_USDT_PCS);
    p.registry.update(&r, |e| e.synced_log = Some(1)).unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        graph_read::current(&p.ctx, &p.registry, &p.tokens, 18).is_ok(),
        "предел не задан — снимок отдаётся"
    );
    set("dex.graph_max_lag_ms", json!(5)).await;
    let e = graph_read::current(&p.ctx, &p.registry, &p.tokens, 18)
        .err()
        .expect("предохранитель");
    assert_eq!(e.code, ErrorCode::StaleData);
    assert!(e.message.starts_with(graph_read::QUIET), "{}", e.message);
    let v = graph_stats::view(&p.ctx, &p.registry);
    assert_eq!(v.fuse_hour, 1, "срабатывание посчитано");
    assert!(v.lag_ms.unwrap() >= 5);
}

#[tokio::test]
async fn owner_publishes_and_follows_registry() {
    let p = route_common::parts_from("tests/data/routes_2026-10-29").await;
    let later = route_common::parts_from("tests/data/routes_2026-10-30").await;
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
    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    let _pools = p.pools(tx);
    let bg = p.registry.graph_cache().bg().clone();
    let wait = |v: u64| {
        let bg = bg.clone();
        async move {
            for _ in 0..500 {
                if bg.load().is_some_and(|s| s.version >= v) && bg.changes.lag().is_none() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("снимок версии {v} не опубликован за 5 с");
        }
    };
    wait(p.registry.version()).await;
    let got = graph::cached(&p.ctx, &p.registry, &p.tokens, 18).unwrap();
    assert_eq!(
        p.registry.graph_cache().builds(),
        0,
        "расчёт граф не строил"
    );
    let old = graph::build(&p.ctx, &p.registry, &p.tokens, 18).unwrap();
    assert_eq!(sets(&got), sets(&old));
    let r = PoolRef::Contract(WBNB_USDT_V2);
    p.registry.put(later.registry.get(&r).unwrap());
    wait(p.registry.version()).await;
    let got = graph::cached(&p.ctx, &p.registry, &p.tokens, 18).unwrap();
    let old = graph::build(&p.ctx, &p.registry, &p.tokens, 18).unwrap();
    assert_eq!(
        sets(&got),
        sets(&old),
        "после изменения пула — та же картина, что build"
    );
    assert_eq!(p.registry.graph_cache().builds(), 0);
}

/// Третье ревью (п. 1): предел ступени Б меньше времени сборки — НЕ петля перезапусков. Предел 1 мс (любая сборка
/// дольше): первое превышение — один перезапуск, повторное в той же ступени снимает льготу, а не владельца; сборка
/// доходит до конца, снимок публикуется, перезапусков не больше одного.
#[tokio::test]
async fn stage_b_limit_does_not_loop() {
    let p = route_common::parts_from("tests/data/routes_2026-10-29").await;
    for (n, v) in [
        ("dex.graph_background", json!(true)),
        ("dex.graph_stage_b_max_ms", json!(1)),
        ("dex.graph_retry_pause_s", json!(1)),
    ] {
        p.ctx
            .settings
            .set(n, Scope::Venue("bsc_dex".into()), Some(v), "тест")
            .await
            .unwrap();
    }
    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    let _pools = p.pools(tx);
    let bg = p.registry.graph_cache().bg().clone();
    let mut ok = false;
    for _ in 0..1000 {
        if bg.load().is_some() {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        ok,
        "снимок опубликован, несмотря на предел ступени Б меньше времени сборки"
    );
    // Четвёртое ревью: после публикации снимка на текущих настройках ступень Б кончилась — отсчёт и признаки сброшены.
    assert!(bg.stage_b_age().is_none(), "отсчёт ступени Б сброшен");
    assert!(
        !bg.stage_b_overdue() && !bg.stage_b_restarted(),
        "признаки ступени Б сброшены"
    );
    let v = graph_stats::view(&p.ctx, &p.registry);
    assert!(
        v.restarts <= 1,
        "перезапусков по пределу ступени Б не больше одного: {}",
        v.restarts
    );
}

/// Четвёртое ревью (п. 1): ступень Б «снимок на прежних настройках» началась, котировка увидела её один раз и больше
/// снимок не читала (как при dex.router_mode = classic) — после публикации снимка на новых настройках отсчёт и признаки
/// сброшены (их ведёт владелец, а не котировки), перезапуска и «просрочки» нет.
#[tokio::test]
async fn stage_b_ends_on_publish_without_quotes() {
    let p = route_common::parts_from("tests/data/routes_2026-10-29").await;
    let set = |n: &'static str, v| {
        let ctx = p.ctx.clone();
        async move {
            ctx.settings
                .set(n, Scope::Venue("bsc_dex".into()), Some(v), "тест")
                .await
                .unwrap();
        }
    };
    set("dex.graph_background", json!(true)).await;
    set("dex.graph_stage_b_max_ms", json!(60000)).await;
    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    let _pools = p.pools(tx);
    let bg = p.registry.graph_cache().bg().clone();
    let until = |want: Vec<String>| {
        let bg = bg.clone();
        async move {
            for _ in 0..1000 {
                if bg.load().is_some_and(|s| s.basis.settings == want) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("снимок на настройках {want:?} не опубликован за 10 с");
        }
    };
    until(graph_settings(&p.ctx)).await;
    // Настройка графа сменилась — владелец видит снимок на прежних настройках (ступень Б) и пересобирает.
    set("dex.route_max_pools_per_edge", json!(4)).await;
    // Одна котировка видит снимок (ступень Б, если новый ещё не готов), дальше котировок нет.
    let _ = graph::cached(&p.ctx, &p.registry, &p.tokens, 18);
    until(graph_settings(&p.ctx)).await;
    assert!(
        bg.stage_b_age().is_none(),
        "отсчёт ступени Б сброшен публикацией"
    );
    assert!(!bg.stage_b_overdue() && !bg.stage_b_restarted());
    let v = graph_stats::view(&p.ctx, &p.registry);
    // Пятое ревью (а): котировка действительно застала ступень Б — иначе тест ничего не проверял бы.
    assert!(v.stale_settings_hour >= 1, "котировка застала ступень Б");
    assert_eq!(v.restarts, 0, "перезапуска нет");
    assert!(v.stage_b_now_ms.is_none() && !v.stage_b_overdue);
}
