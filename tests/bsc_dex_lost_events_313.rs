//! Файл: tests/bsc_dex_lost_events_313.rs
//! Что делает: решение 313 (ревью) — чистая логика без данных площадок:
//! - pools/lost_events.rs: счёт потерь (Lost::add / count); часы записи (Clock::due) — первая запись сразу, внутри
//!   интервала записи нет и настройка не читается, после интервала — запись с прошедшим временем, интервал не задан —
//!   записей нет никогда;
//! - node/req_trail.rs: record вне scope ничего не пишет, внутри scope — запросы по порядку, take забирает и очищает.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::lost_events, arb::venues::bsc_dex::node::req_trail.
//! Настройки: нет.

use std::cell::Cell;
use std::time::{Duration, Instant};

use arb::venues::bsc_dex::node::req_trail::{self, Step, Trail};
use arb::venues::bsc_dex::pools::lost_events::{Clock, Lost};

#[test]
fn lost_counts_every_loss() {
    let lost = Lost::default();
    assert_eq!(lost.count(), 0);
    lost.add();
    lost.add();
    lost.clone().add();
    assert_eq!(lost.count(), 3, "клон считает в тот же счётчик");
}

#[test]
fn first_record_at_once_then_not_within_interval() {
    let mut c = Clock::default();
    let t0 = Instant::now();
    let every = Duration::from_secs(60);
    assert_eq!(
        c.due(t0, || Some(every)),
        Some(None),
        "первая запись — сразу"
    );
    let reads = Cell::new(0);
    let read = || {
        reads.set(reads.get() + 1);
        Some(every)
    };
    assert_eq!(c.due(t0 + Duration::from_secs(10), read), None);
    assert_eq!(
        reads.get(),
        0,
        "внутри прочитанного интервала настройка не читается"
    );
    let later = t0 + Duration::from_secs(61);
    assert_eq!(
        c.due(later, || Some(every)),
        Some(Some(Duration::from_secs(61)))
    );
    assert_eq!(c.due(later + Duration::from_secs(1), || Some(every)), None);
}

#[test]
fn not_set_interval_never_records() {
    let mut c = Clock::default();
    let t0 = Instant::now();
    for s in [0, 1, 100, 10_000] {
        assert_eq!(c.due(t0 + Duration::from_secs(s), || None), None);
    }
    // Задали позже — первая запись сразу.
    let at = t0 + Duration::from_secs(10_001);
    assert_eq!(c.due(at, || Some(Duration::from_secs(5))), Some(None));
}

#[test]
fn longer_interval_is_reread_when_old_one_passed() {
    let mut c = Clock::default();
    let t0 = Instant::now();
    assert!(c.due(t0, || Some(Duration::from_secs(5))).is_some());
    // Прошёл старый интервал, новое значение больше — записи ещё нет.
    let t = t0 + Duration::from_secs(6);
    assert_eq!(c.due(t, || Some(Duration::from_secs(60))), None);
    assert_eq!(
        c.due(t0 + Duration::from_secs(60), || Some(Duration::from_secs(
            60
        ))),
        Some(Some(Duration::from_secs(60)))
    );
}

#[tokio::test]
async fn trail_outside_scope_records_nothing_inside_in_order() {
    // Вне scope — без списка, без паники.
    req_trail::record(1, Duration::from_millis(1), Duration::from_millis(2));
    let trail = Trail::default();
    req_trail::scope(trail.clone(), async {
        req_trail::record(1, Duration::from_millis(3), Duration::from_millis(4));
        req_trail::record(5, Duration::from_millis(6), Duration::from_millis(7));
    })
    .await;
    // После scope — снова без списка.
    req_trail::record(9, Duration::from_millis(9), Duration::from_millis(9));
    let got = trail.take();
    assert_eq!(
        got,
        vec![
            Step {
                calls: 1,
                wait: Duration::from_millis(3),
                send: Duration::from_millis(4),
            },
            Step {
                calls: 5,
                wait: Duration::from_millis(6),
                send: Duration::from_millis(7),
            },
        ]
    );
    assert!(trail.take().is_empty(), "take забирает и очищает");
}
