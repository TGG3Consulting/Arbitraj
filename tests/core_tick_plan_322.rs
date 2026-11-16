//! Файл: tests/core_tick_plan_322.rs
//! Что делает: решение 322 — событие пула пересчитывается сразу (signal.pool_event_now). Чистая логика плана
//! пересчётов (core/tick_plan.rs Plan): время передаётся параметром (без sleep). Проверяет: (1) не задана — прежний
//! порядок решения 109 (тики в повторе, повтор — после срока сна, без «первых»); (2) включено: событие пула во время сна
//! будит задачу и даёт пересчёт сразу, направления его площадки первыми; во время идущего пересчёта — не прерывает
//! (в повтор, пересчёт сразу после); тик стакана сна не будит; (3) ограничитель signal.pool_event_min_interval_ms;
//! (4) без signal.eval_min_interval_ms — как раньше; (5) отметка события пула (время, блок) едет с пересчётом;
//! (6) ревью 16.11: сигнал пробуждения из плана будит сон задачи монеты (тот же tokio::select! sleep / Notify, что в
//! core/tick_sched.rs task; без ожидания — один опрос future), во время идущего пересчёта сигнала нет; направления
//! площадки события пула первыми (core/evaluate.rs pool_first, устойчиво).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::tick_plan, arb::core::evaluate (pool_first), arb::core::types, tokio, futures-util.
//! Моков нет: площадки не участвуют.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arb::core::evaluate::pool_first;
use arb::core::tick_plan::{Cfg, Next, Plan, PoolMark, Put, Run};
use arb::core::types::{Asset, Direction, VenueId};
use chrono::Utc;
use futures_util::FutureExt;
use tokio::sync::Notify;

fn coin() -> Asset {
    Asset {
        ticker: "SHOPINX".into(),
        bsc_contract: None,
    }
}

fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

fn set(ids: &[&str]) -> HashSet<VenueId> {
    ids.iter().map(|i| v(i)).collect()
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Живой бот 16.11: signal.eval_min_interval_ms 500.
fn cfg(pool_now: bool, pool_min: Option<u64>) -> Cfg {
    Cfg {
        min: Some(ms(500)),
        pool_now,
        pool_min: pool_min.map(ms),
    }
}

fn mark(block: u64) -> PoolMark {
    PoolMark {
        at: Utc::now(),
        block,
    }
}

/// Первый пересчёт по стакану биржи в t0, задача уснула до t0 + 500 мс.
fn sleeping(p: &mut Plan, c: Cfg, t0: Instant) {
    assert!(matches!(
        p.put(&coin(), set(&["bingx"]), None, c.pool_now, t0),
        Put::Spawn(_)
    ));
    assert_eq!(
        p.next(&coin(), Some(t0 + ms(500)), c, t0 + ms(10)),
        Next::Sleep(t0 + ms(500))
    );
}

#[test]
fn off_as_before() {
    let (mut p, t0, c) = (Plan::default(), Instant::now(), cfg(false, None));
    sleeping(&mut p, c, t0);
    let m = mark(133910892);
    assert!(matches!(
        p.put(&coin(), set(&["bsc_dex"]), Some(m), false, t0 + ms(20)),
        Put::Queued
    ));
    let dl = Some(t0 + ms(500));
    assert_eq!(
        p.next(&coin(), dl, c, t0 + ms(20)),
        Next::Sleep(t0 + ms(500))
    );
    let go = Run {
        venues: set(&["bsc_dex"]),
        first: HashSet::new(),
        pool: Some(m),
    };
    assert_eq!(p.next(&coin(), dl, c, t0 + ms(500)), Next::Go(go));
    // Повтор пуст, срок прошёл — задача кончается.
    assert_eq!(
        p.next(&coin(), Some(t0 + ms(1000)), c, t0 + ms(1000)),
        Next::Done
    );
}

#[test]
fn pool_event_wakes_sleep() {
    let (mut p, t0, c) = (Plan::default(), Instant::now(), cfg(true, None));
    sleeping(&mut p, c, t0);
    let m = mark(133910892);
    assert!(matches!(
        p.put(&coin(), set(&["bsc_dex"]), Some(m), true, t0 + ms(20)),
        Put::Wake(_)
    ));
    let go = Run {
        venues: set(&["bsc_dex"]),
        first: set(&["bsc_dex"]),
        pool: Some(m),
    };
    assert_eq!(
        p.next(&coin(), Some(t0 + ms(500)), c, t0 + ms(21)),
        Next::Go(go)
    );
}

#[test]
fn book_tick_does_not_wake() {
    let (mut p, t0, c) = (Plan::default(), Instant::now(), cfg(true, None));
    sleeping(&mut p, c, t0);
    assert!(matches!(
        p.put(&coin(), set(&["bingx"]), None, true, t0 + ms(20)),
        Put::Queued
    ));
    assert_eq!(
        p.next(&coin(), Some(t0 + ms(500)), c, t0 + ms(21)),
        Next::Sleep(t0 + ms(500))
    );
}

#[test]
fn running_is_not_interrupted() {
    let (mut p, t0, c) = (Plan::default(), Instant::now(), cfg(true, None));
    p.put(&coin(), set(&["bingx"]), None, true, t0);
    // Пересчёт идёт (задача не спит) — событие только в повтор, будить нечего.
    let m = mark(1);
    assert!(matches!(
        p.put(&coin(), set(&["bsc_dex"]), Some(m), true, t0 + ms(100)),
        Put::Queued
    ));
    // Пересчёт кончился в 300 мс — следующий сразу, не ждёт срока 500 мс.
    let go = Run {
        venues: set(&["bsc_dex"]),
        first: set(&["bsc_dex"]),
        pool: Some(m),
    };
    assert_eq!(
        p.next(&coin(), Some(t0 + ms(500)), c, t0 + ms(300)),
        Next::Go(go)
    );
}

#[test]
fn limiter_holds_pool_runs() {
    let (mut p, t0, c) = (Plan::default(), Instant::now(), cfg(true, Some(300)));
    // Первый пересчёт — от события пула в t0.
    p.put(&coin(), set(&["bsc_dex"]), Some(mark(1)), true, t0);
    let dl = Some(t0 + ms(500));
    assert_eq!(
        p.next(&coin(), dl, c, t0 + ms(10)),
        Next::Sleep(t0 + ms(500))
    );
    assert!(matches!(
        p.put(
            &coin(),
            set(&["bsc_dex"]),
            Some(mark(2)),
            true,
            t0 + ms(100)
        ),
        Put::Wake(_)
    ));
    // Ограничитель 300 мс от начала прошлого пересчёта с событием пула: спать до t0 + 300, не до t0 + 500.
    assert_eq!(
        p.next(&coin(), dl, c, t0 + ms(100)),
        Next::Sleep(t0 + ms(300))
    );
    assert!(matches!(p.next(&coin(), dl, c, t0 + ms(300)), Next::Go(_)));
}

#[test]
fn no_min_interval_as_before() {
    let mut p = Plan::default();
    let (t0, c) = (Instant::now(), Cfg::default());
    p.put(&coin(), set(&["bingx"]), None, false, t0);
    p.put(&coin(), set(&["mexc"]), None, false, t0);
    let go = Run {
        venues: set(&["mexc"]),
        ..Default::default()
    };
    assert_eq!(p.next(&coin(), None, c, t0), Next::Go(go));
    assert_eq!(p.next(&coin(), None, c, t0), Next::Done);
    // Задачи нет — новый тик снова запускает задачу.
    assert!(matches!(
        p.put(&coin(), set(&["bingx"]), None, false, t0),
        Put::Spawn(_)
    ));
}

#[test]
fn first_pool_mark_kept_in_merge() {
    let (mut p, t0, c) = (Plan::default(), Instant::now(), cfg(false, None));
    sleeping(&mut p, c, t0);
    let (m1, m2) = (mark(10), mark(11));
    p.put(&coin(), set(&["bsc_dex"]), Some(m1), false, t0 + ms(20));
    p.put(&coin(), set(&["bsc_dex"]), Some(m2), false, t0 + ms(30));
    p.put(&coin(), set(&["bingx"]), None, false, t0 + ms(40));
    match p.next(&coin(), Some(t0 + ms(500)), c, t0 + ms(500)) {
        Next::Go(r) => {
            assert_eq!(r.pool, Some(m1));
            assert_eq!(r.venues, set(&["bsc_dex", "bingx"]));
        }
        n => panic!("ждали пересчёт, получили {n:?}"),
    }
}

/// Сон задачи монеты как в core/tick_sched.rs task: срок сна (здесь — час) или сигнал пробуждения. Один опрос без
/// ожидания: Some(true) — разбужен сигналом, None — спит.
fn poll_sleep(w: &Arc<Notify>) -> Option<bool> {
    async {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(3600)) => false,
            _ = w.notified() => true,
        }
    }
    .now_or_never()
}

#[tokio::test]
async fn wake_signal_ends_task_sleep() {
    let (mut p, t0, c) = (Plan::default(), Instant::now(), cfg(true, None));
    sleeping(&mut p, c, t0);
    // Задача взяла сигнал пробуждения перед сном (tick_sched.rs: t.wake) и спит.
    let w = p.wake(&coin()).expect("задача монеты есть");
    assert_eq!(poll_sleep(&w), None);
    // Событие пула — план отдаёт тот же сигнал, tick_sched.rs put будит (notify_one) — сон кончается сразу.
    let Put::Wake(n) = p.put(&coin(), set(&["bsc_dex"]), Some(mark(7)), true, t0 + ms(20)) else {
        panic!("ждали пробуждение");
    };
    assert!(Arc::ptr_eq(&w, &n));
    n.notify_one();
    assert_eq!(poll_sleep(&w), Some(true));
}

#[tokio::test]
async fn running_gets_no_wake_signal() {
    let (mut p, t0, c) = (Plan::default(), Instant::now(), cfg(true, None));
    p.put(&coin(), set(&["bingx"]), None, true, t0);
    let w = p.wake(&coin()).expect("задача монеты есть");
    // Пересчёт идёт — событие пула только в повтор, сигнала нет: идущий пересчёт не прерывается.
    assert!(matches!(
        p.put(
            &coin(),
            set(&["bsc_dex"]),
            Some(mark(8)),
            true,
            t0 + ms(100)
        ),
        Put::Queued
    ));
    assert_eq!(poll_sleep(&w), None);
    // Следующий шаг задачи после пересчёта — сразу повтор с событием пула.
    assert!(matches!(
        p.next(&coin(), Some(t0 + ms(500)), c, t0 + ms(300)),
        Next::Go(_)
    ));
}

fn dir(a: &str, b: &str) -> Direction {
    Direction {
        asset: coin(),
        buy_on: v(a),
        sell_on: v(b),
    }
}

#[test]
fn pool_venue_directions_first_stable() {
    let start = vec![
        dir("bingx", "mexc"),
        dir("bsc_dex", "bingx"),
        dir("mexc", "bingx"),
        dir("bingx", "bsc_dex"),
    ];
    let mut d = start.clone();
    pool_first(&mut d, &set(&["bsc_dex"]));
    assert_eq!(
        d,
        vec![
            dir("bsc_dex", "bingx"),
            dir("bingx", "bsc_dex"),
            dir("bingx", "mexc"),
            dir("mexc", "bingx"),
        ]
    );
    // Площадок события пула нет — порядок прежний.
    let mut d = start.clone();
    pool_first(&mut d, &HashSet::new());
    assert_eq!(d, start);
}
