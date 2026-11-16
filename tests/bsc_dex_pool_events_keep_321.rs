//! Файл: tests/bsc_dex_pool_events_keep_321.rs
//! Что делает: решение 321 — набор «ждут доставки» на настоящем канале tokio малой ёмкости, чистая логика без данных
//! площадок (событие — пара (пул, номер состояния)). С решения 324 набор живёт в шлюзе (gateway/keep_last.rs, перенесён
//! из venues/bsc_dex/pools/pending_events.rs) — проверяется тем же поведением 321:
//! - канал полон, откладывать велено — события откладываются, не теряются; одно на пул (новое заменяет прежнее);
//! - место появилось — доставлены все ждущие, по одному на пул, в порядке откладывания, последнее состояние каждого;
//!   доставщик завершается сам (без новых событий);
//! - пока кто-то ждёт, новое событие встаёт за ждущими (сначала ждущие), настройка не читается;
//! - пул «не живой» при доставке (недостоверен / снят) — не отправляется;
//! - откладывать не велено (настройка не задана) — прежний сброс (Lost), набор пуст; при свободном канале настройка
//!   не читается; канал закрыт — Closed.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::gateway::keep_last, tokio.
//! Настройки: нет.

use std::cell::Cell;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use arb::gateway::keep_last::{Counts, Keyed, Offer, Pending, Slot, Take};
use tokio::sync::mpsc;

type Ev = (u32, u64);
/// Пулы, ставшие «не живыми» (как недостоверный пул в реестре DEX).
type Dead = Arc<Mutex<HashSet<u32>>>;

/// Ключ — пул; класс один (пулы одной площадки).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct P(u32);

impl Keyed for P {
    type Class = u8;
    fn class(&self) -> u8 {
        0
    }
}

/// Слот как у пула DEX: последнее событие, проверка «живой» при доставке.
struct E(Ev, Dead);

impl Slot for E {
    type Msg = Ev;
    fn merge(&mut self, newer: Self) -> bool {
        *self = newer;
        true
    }
    fn now(self) -> Ev {
        self.0
    }
    fn take(self) -> Take<Ev, Self> {
        let dead = self.1.lock().expect("замок").contains(&self.0.0);
        if dead {
            Take::Skip
        } else {
            Take::Send(self.0, None)
        }
    }
    fn urgent(&self) -> bool {
        false
    }
}

fn e(d: &Dead, v: Ev) -> E {
    E(v, d.clone())
}

#[tokio::test]
async fn full_channel_defers_then_delivers_one_per_pool_in_order() {
    let (tx, mut rx) = mpsc::channel::<Ev>(1);
    let set: Pending<P, E> = Pending::default();
    let d = Dead::default();
    // Свободно — сразу в канал, настройка не читается.
    let read = Cell::new(0);
    let keep = || {
        read.set(read.get() + 1);
        true
    };
    assert_eq!(set.offer(&tx, P(7), e(&d, (7, 1)), keep), Offer::Sent);
    assert_eq!(read.get(), 0, "при свободном канале настройка не читается");
    // Канал полон — откладываем.
    let r = set.offer(&tx, P(1), e(&d, (1, 1)), || true);
    assert_eq!(r, Offer::Deferred { start: true });
    let r = set.offer(&tx, P(2), e(&d, (2, 1)), || true);
    assert_eq!(r, Offer::Deferred { start: false });
    let r = set.offer(&tx, P(1), e(&d, (1, 2)), || true);
    assert_eq!(r, Offer::Replaced);
    // Пока ждут — за ждущими без чтения настройки (|| false не вызывается).
    let r = set.offer(&tx, P(3), e(&d, (3, 1)), || false);
    assert_eq!(r, Offer::Deferred { start: false });
    assert_eq!(set.waiting(), 3);
    let worker = tokio::spawn(set.clone().deliver(tx.clone(), ()));
    let mut got = vec![];
    for _ in 0..4 {
        got.push(rx.recv().await.expect("событие"));
    }
    assert_eq!(got, vec![(7, 1), (1, 2), (2, 1), (3, 1)]);
    assert!(worker.await.expect("доставщик"), "набор опустел");
    let want = Counts {
        deferred: 3,
        replaced: 1,
        delivered: 3,
        dropped: 0,
        waiting: 0,
    };
    assert_eq!(set.take(&0), want);
    assert!(rx.try_recv().is_err(), "лишних событий нет");
}

#[tokio::test]
async fn waiting_first_then_fresh_and_dead_pool_dropped() {
    let (tx, mut rx) = mpsc::channel::<Ev>(1);
    let set: Pending<P, E> = Pending::default();
    let d = Dead::default();
    assert_eq!(set.offer(&tx, P(9), e(&d, (9, 1)), || true), Offer::Sent);
    let r = set.offer(&tx, P(1), e(&d, (1, 1)), || true);
    assert_eq!(r, Offer::Deferred { start: true });
    let r = set.offer(&tx, P(2), e(&d, (2, 1)), || true);
    assert_eq!(r, Offer::Deferred { start: false });
    // Ядро забрало событие — место есть, но ждущий 1 есть: свежий 4 встаёт за ним, а не обгоняет.
    assert_eq!(rx.recv().await, Some((9, 1)));
    let r = set.offer(&tx, P(4), e(&d, (4, 1)), || true);
    assert_eq!(r, Offer::Deferred { start: false });
    // Пул 2 стал недостоверным, пока ждал.
    d.lock().expect("замок").insert(2);
    let worker = tokio::spawn(set.clone().deliver(tx.clone(), ()));
    assert_eq!(rx.recv().await, Some((1, 1)));
    assert_eq!(rx.recv().await, Some((4, 1)));
    assert!(worker.await.expect("доставщик"));
    let c = set.take(&0);
    assert_eq!((c.delivered, c.dropped, c.waiting), (2, 1, 0));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn not_enabled_is_old_drop_and_closed_is_closed() {
    let (tx, mut rx) = mpsc::channel::<Ev>(1);
    let set: Pending<P, E> = Pending::default();
    let d = Dead::default();
    assert_eq!(set.offer(&tx, P(1), e(&d, (1, 1)), || false), Offer::Sent);
    assert_eq!(set.offer(&tx, P(2), e(&d, (2, 1)), || false), Offer::Lost);
    assert_eq!(set.waiting(), 0, "без настройки ничего не ждёт");
    // Потери считает вызывающий (ревью 324, Б2: без замка набора) — у набора счёта потерь нет.
    assert_eq!(rx.recv().await, Some((1, 1)));
    assert!(rx.try_recv().is_err(), "потерянное не пришло");
    drop(rx);
    assert_eq!(set.offer(&tx, P(3), e(&d, (3, 1)), || true), Offer::Closed);
}

#[tokio::test]
async fn closed_channel_clears_set_and_stops_worker() {
    let (tx, rx) = mpsc::channel::<Ev>(1);
    let set: Pending<P, E> = Pending::default();
    let d = Dead::default();
    assert_eq!(set.offer(&tx, P(1), e(&d, (1, 1)), || true), Offer::Sent);
    let r = set.offer(&tx, P(2), e(&d, (2, 1)), || true);
    assert_eq!(r, Offer::Deferred { start: true });
    drop(rx);
    let done = set.clone().deliver(tx.clone(), ()).await;
    assert!(!done, "канал закрыт");
    assert_eq!(set.waiting(), 0, "мёртвое не держим");
    // Следующее отложенное снова запускает доставщика.
    let (tx2, _rx2) = mpsc::channel::<Ev>(1);
    assert_eq!(set.offer(&tx2, P(5), e(&d, (5, 1)), || true), Offer::Sent);
    let r = set.offer(&tx2, P(6), e(&d, (6, 1)), || true);
    assert_eq!(r, Offer::Deferred { start: true });
}
