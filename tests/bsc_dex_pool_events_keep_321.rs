//! Файл: tests/bsc_dex_pool_events_keep_321.rs
//! Что делает: решение 321 — набор «ждут доставки» (pools/pending_events.rs) на настоящем канале tokio малой ёмкости,
//! чистая логика без данных площадок (событие — пара (пул, номер состояния)):
//! - канал полон, откладывать велено — события откладываются, не теряются; одно на пул (новое заменяет прежнее);
//! - место появилось — доставлены все ждущие, по одному на пул, в порядке откладывания, последнее состояние каждого;
//!   доставщик завершается сам (без новых событий);
//! - пока кто-то ждёт, новое событие встаёт за ждущими (сначала ждущие);
//! - пул «не живой» при доставке (недостоверен / снят) — не отправляется;
//! - откладывать не велено (настройка не задана) — прежний сброс (Lost), набор пуст; при свободном канале настройка
//!   не читается; канал закрыт — Closed.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::pending_events, tokio.
//! Настройки: нет.

use std::cell::Cell;

use arb::venues::bsc_dex::pools::pending_events::{Counts, Offer, Pending};
use tokio::sync::mpsc;

type Ev = (u32, u64);

fn id(e: Ev) -> Ev {
    e
}

#[tokio::test]
async fn full_channel_defers_then_delivers_one_per_pool_in_order() {
    let (tx, mut rx) = mpsc::channel::<Ev>(1);
    let set: Pending<u32, Ev> = Pending::default();
    // Свободно — сразу в канал, настройка не читается.
    let read = Cell::new(0);
    let keep = || {
        read.set(read.get() + 1);
        true
    };
    assert_eq!(set.offer(&tx, 7, (7, 1), id, keep), Offer::Sent);
    assert_eq!(read.get(), 0, "при свободном канале настройка не читается");
    // Канал полон — откладываем.
    assert_eq!(
        set.offer(&tx, 1, (1, 1), id, || true),
        Offer::Deferred { start: true }
    );
    assert_eq!(
        set.offer(&tx, 2, (2, 1), id, || true),
        Offer::Deferred { start: false }
    );
    assert_eq!(set.offer(&tx, 1, (1, 2), id, || true), Offer::Replaced);
    assert_eq!(
        set.offer(&tx, 3, (3, 1), id, || false),
        Offer::Deferred { start: false }
    );
    assert_eq!(set.waiting(), 3);
    let worker = tokio::spawn(set.clone().deliver(tx.clone(), id, |_| true));
    let mut got = vec![];
    for _ in 0..4 {
        got.push(rx.recv().await.expect("событие"));
    }
    assert_eq!(got, vec![(7, 1), (1, 2), (2, 1), (3, 1)]);
    assert!(worker.await.expect("доставщик"), "набор опустел");
    assert_eq!(
        set.take(),
        Counts {
            deferred: 3,
            replaced: 1,
            delivered: 3,
            dropped: 0,
            waiting: 0,
        }
    );
    assert!(rx.try_recv().is_err(), "лишних событий нет");
}

#[tokio::test]
async fn waiting_first_then_fresh_and_dead_pool_dropped() {
    let (tx, mut rx) = mpsc::channel::<Ev>(1);
    let set: Pending<u32, Ev> = Pending::default();
    assert_eq!(set.offer(&tx, 9, (9, 1), id, || true), Offer::Sent);
    assert_eq!(
        set.offer(&tx, 1, (1, 1), id, || true),
        Offer::Deferred { start: true }
    );
    assert_eq!(
        set.offer(&tx, 2, (2, 1), id, || true),
        Offer::Deferred { start: false }
    );
    // Ядро забрало событие — место есть, но ждущий 1 есть: свежий 4 встаёт за ним, а не обгоняет.
    assert_eq!(rx.recv().await, Some((9, 1)));
    assert_eq!(
        set.offer(&tx, 4, (4, 1), id, || true),
        Offer::Deferred { start: false }
    );
    // Пул 2 стал недостоверным, пока ждал.
    let worker = tokio::spawn(set.clone().deliver(tx.clone(), id, |k| *k != 2));
    assert_eq!(rx.recv().await, Some((1, 1)));
    assert_eq!(rx.recv().await, Some((4, 1)));
    assert!(worker.await.expect("доставщик"));
    let c = set.take();
    assert_eq!((c.delivered, c.dropped, c.waiting), (2, 1, 0));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn not_enabled_is_old_drop_and_closed_is_closed() {
    let (tx, mut rx) = mpsc::channel::<Ev>(1);
    let set: Pending<u32, Ev> = Pending::default();
    assert_eq!(set.offer(&tx, 1, (1, 1), id, || false), Offer::Sent);
    assert_eq!(set.offer(&tx, 2, (2, 1), id, || false), Offer::Lost);
    assert_eq!(set.waiting(), 0, "без настройки ничего не ждёт");
    assert_eq!(rx.recv().await, Some((1, 1)));
    assert!(rx.try_recv().is_err(), "потерянное не пришло");
    drop(rx);
    assert_eq!(set.offer(&tx, 3, (3, 1), id, || true), Offer::Closed);
}

#[tokio::test]
async fn closed_channel_clears_set_and_stops_worker() {
    let (tx, rx) = mpsc::channel::<Ev>(1);
    let set: Pending<u32, Ev> = Pending::default();
    assert_eq!(set.offer(&tx, 1, (1, 1), id, || true), Offer::Sent);
    assert_eq!(
        set.offer(&tx, 2, (2, 1), id, || true),
        Offer::Deferred { start: true }
    );
    drop(rx);
    let done = set.clone().deliver(tx.clone(), id, |_| true).await;
    assert!(!done, "канал закрыт");
    assert_eq!(set.waiting(), 0, "мёртвое не держим");
    // Следующее отложенное снова запускает доставщика.
    let (tx2, _rx2) = mpsc::channel::<Ev>(1);
    assert_eq!(set.offer(&tx2, 5, (5, 1), id, || true), Offer::Sent);
    assert_eq!(
        set.offer(&tx2, 6, (6, 1), id, || true),
        Offer::Deferred { start: true }
    );
}
