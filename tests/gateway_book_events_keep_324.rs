//! Файл: tests/gateway_book_events_keep_324.rs
//! Что делает: решение 324 — общий набор канала ядра (gateway/core_out.rs) и помощник бирж (gateway/core_out_book.rs) на
//! настоящем канале tokio малой ёмкости и настоящем хранилище настроек (SQLite во временной папке); данных площадок нет:
//! стаканы — пустые книги с разным временем получения (по нему и узнаём кадр), событие пула в тесте очереди — метка
//! (MarketEvent::Notice с именем пула: порядок набора от вида события не зависит).
//! - venue.book_events_keep = да: канал полон — по паре ждёт последний кадр, received_at не меняется;
//! - пулы и стаканы — одна честная очередь (FIFO по ключам): повторные события ждущего ключа место не двигают, никто
//!   не голодает; набор один на канал (клоны Sender — тот же набор);
//! - Invalidated не теряется (и без настройки) и не обходится старым кадром: ждавший кадр пары снят, кадр после
//!   Invalidated уходит следом;
//! - настройка не задана — прежний сброс (Lost), даже когда в наборе ждут пулы и когда ждёт кадр после Invalidated
//!   (ревью 324, Б3);
//! - guard доставщика: паника / снятие задачи — следующий отложенный снова запускает доставщика.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::gateway::{core_out, core_out_book, core_out_item, keep_last}, arb::settings, arb::storage, tokio.
//! Настройки: venue.book_events_keep (задаётся тестом в своей временной базе).

use alloy_primitives::Address;
use arb::core::types::{Asset, OrderBook, PoolRef, VenueId};
use arb::gateway::core_out;
use arb::gateway::core_out_book::{self, BookOut, Fed};
use arb::gateway::core_out_item::{Item, Key};
use arb::gateway::events::MarketEvent;
use arb::gateway::keep_last::{Keyed, Offer, Pending, Slot, Take};
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use serde_json::json;
use tokio::sync::{broadcast, mpsc};

/// Новая база во временной папке: настройки и журнал. Ошибок нет (паника теста).
async fn fresh(name: &str) -> (Store, Journal) {
    let dir = std::env::temp_dir().join(format!("arb-t324-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("папка");
    let db = Db::open(&dir.join("t.sqlite")).expect("база");
    let (tx, _) = broadcast::channel(16);
    let (jtx, _) = broadcast::channel(16);
    let journal = Journal::new(db.clone(), jtx);
    let store = Store::load(db, tx, journal.clone())
        .await
        .expect("настройки");
    (store, journal)
}

/// Включает venue.book_events_keep у площадки.
async fn keep_on(s: &Store, venue: &str) {
    let scope = Scope::Venue(venue.into());
    s.set("venue.book_events_keep", scope, Some(json!(true)), "тест")
        .await
        .expect("настройка");
}

fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Пустая книга пары; ms — время получения (метка кадра).
fn book(venue: &str, ticker: &str, ms: i64) -> OrderBook {
    OrderBook {
        venue: v(venue),
        asset: Asset {
            ticker: ticker.into(),
            bsc_contract: None,
        },
        bids: vec![],
        asks: vec![],
        exchange_time: None,
        received_at: chrono::DateTime::from_timestamp_millis(ms).expect("время"),
    }
}

/// Метка события: «A@2» — кадр A с received_at 2 мс, «inv A», имя пула.
fn tag(ev: &MarketEvent) -> String {
    match ev {
        MarketEvent::Book(b) => format!("{}@{}", b.asset.ticker, b.received_at.timestamp_millis()),
        MarketEvent::Invalidated { ticker, .. } => format!("inv {ticker}"),
        MarketEvent::Notice { text, .. } => text.clone(),
        _ => "?".into(),
    }
}

async fn recv(rx: &mut mpsc::Receiver<MarketEvent>, n: usize) -> Vec<String> {
    let mut out = vec![];
    for _ in 0..n {
        out.push(tag(&rx.recv().await.expect("событие")));
    }
    out
}

/// Ключ и событие-метка пула.
fn pool(n: u8) -> (Key, Item) {
    let key = Key::Pool(v("bsc_dex"), PoolRef::Contract(Address::repeat_byte(n)));
    let ev = MarketEvent::Notice {
        venue: v("bsc_dex"),
        code: "pool".into(),
        text: format!("P{n}"),
    };
    (key, Item::Event(ev))
}

#[tokio::test]
async fn book_keeps_last_frame_per_pair() {
    let (s, j) = fresh("last").await;
    keep_on(&s, "bingx").await;
    let (tx, mut rx) = mpsc::channel::<MarketEvent>(1);
    let out = BookOut::new(&tx, v("bingx"), &s, &j);
    assert_eq!(out.book(&tx, book("bingx", "A", 1)), Fed::Sent);
    assert_eq!(out.book(&tx, book("bingx", "A", 2)), Fed::Kept);
    assert_eq!(out.book(&tx, book("bingx", "B", 1)), Fed::Kept);
    assert_eq!(out.book(&tx, book("bingx", "A", 3)), Fed::Kept);
    assert_eq!(
        core_out::of(&tx.clone()).waiting(),
        2,
        "по паре — один кадр; набор один на канал"
    );
    assert_eq!(
        recv(&mut rx, 3).await,
        ["A@1", "A@3", "B@1"],
        "последний кадр пары, время не подменено"
    );
    tokio::task::yield_now().await;
    assert!(rx.try_recv().is_err(), "промежуточный кадр A@2 не пришёл");
}

#[tokio::test]
async fn pools_and_books_one_fair_queue() {
    let (tx, mut rx) = mpsc::channel::<MarketEvent>(1);
    let feed = core_out::of(&tx);
    let book_item = |t: &str, ms| Item::Event(MarketEvent::Book(book("mexc", t, ms)));
    let bk = |t: &str| Key::Book(v("mexc"), t.into());
    let (k, i) = pool(1);
    assert_eq!(feed.offer(&tx, k, i, || true), Offer::Sent);
    let (k, i) = pool(2);
    assert_eq!(
        feed.offer(&tx, k, i, || true),
        Offer::Deferred { start: true }
    );
    assert_eq!(
        feed.offer(&tx, bk("A"), book_item("A", 1), || true),
        Offer::Deferred { start: false }
    );
    let (k, i) = pool(3);
    assert_eq!(
        feed.offer(&tx, k, i, || true),
        Offer::Deferred { start: false }
    );
    // Повторные события ждущего пула 2 место в очереди не двигают — стаканы за ним не голодают.
    for _ in 0..3 {
        let (k, i) = pool(2);
        assert_eq!(feed.offer(&tx, k, i, || true), Offer::Replaced);
    }
    assert_eq!(
        feed.offer(&tx, bk("B"), book_item("B", 1), || true),
        Offer::Deferred { start: false }
    );
    assert_eq!(recv(&mut rx, 5).await, ["P1", "P2", "A@1", "P3", "B@1"]);
}

#[tokio::test]
async fn invalidated_not_lost_and_not_overtaken_by_old_frame() {
    let (s, j) = fresh("inv").await;
    keep_on(&s, "gate").await;
    let (tx, mut rx) = mpsc::channel::<MarketEvent>(1);
    let out = BookOut::new(&tx, v("gate"), &s, &j);
    assert_eq!(out.book(&tx, book("gate", "A", 1)), Fed::Sent);
    assert_eq!(out.book(&tx, book("gate", "A", 2)), Fed::Kept);
    assert_eq!(out.book(&tx, book("gate", "B", 1)), Fed::Kept);
    // Адаптер объявил стакан A недостоверным: ждавший старый A@2 снят, «недостоверно» — в начало очереди.
    core_out_book::invalidated(&tx, v("gate"), "A");
    // Кадр после Invalidated — уходит следом за ним, а не раньше.
    assert_eq!(out.book(&tx, book("gate", "A", 3)), Fed::Kept);
    assert_eq!(recv(&mut rx, 4).await, ["A@1", "inv A", "A@3", "B@1"]);
}

#[tokio::test]
async fn invalidated_kept_without_setting() {
    let (s, j) = fresh("inv-off").await;
    let (tx, mut rx) = mpsc::channel::<MarketEvent>(1);
    let out = BookOut::new(&tx, v("okx"), &s, &j);
    assert_eq!(out.book(&tx, book("okx", "A", 1)), Fed::Sent);
    core_out_book::invalidated(&tx, v("okx"), "A");
    assert_eq!(
        core_out::of(&tx).waiting(),
        1,
        "канал полон — Invalidated ждёт, не потерян"
    );
    assert_eq!(recv(&mut rx, 2).await, ["A@1", "inv A"]);
}

#[tokio::test]
async fn not_set_is_old_drop_even_when_pools_wait() {
    let (s, j) = fresh("off").await;
    let (tx, mut rx) = mpsc::channel::<MarketEvent>(1);
    let out = BookOut::new(&tx, v("kraken"), &s, &j);
    assert_eq!(out.book(&tx, book("kraken", "A", 1)), Fed::Sent);
    assert_eq!(
        out.book(&tx, book("kraken", "B", 1)),
        Fed::Lost,
        "канал полон — как раньше"
    );
    // В наборе ждёт пул DEX — стакан без настройки всё равно не откладывается.
    let (k, i) = pool(1);
    let feed = core_out::of(&tx);
    assert_eq!(
        feed.offer(&tx, k, i, || true),
        Offer::Deferred { start: true }
    );
    assert_eq!(out.book(&tx, book("kraken", "C", 1)), Fed::Lost);
    assert_eq!(feed.waiting(), 1, "ждёт только пул");
    assert_eq!(recv(&mut rx, 2).await, ["A@1", "P1"]);
    tokio::task::yield_now().await;
    assert!(rx.try_recv().is_err(), "потерянные кадры не пришли");
}

/// Ревью 324 (Б3): кадр, пришедший после Invalidated (остаток срочного слота), ждёт «вынужденным» — без настройки
/// класс стаканов площадки НЕ уходит в режим «откладывать»: новая пара при полном канале теряется, как раньше.
#[tokio::test]
async fn rest_after_invalidated_does_not_hold_venue_without_setting() {
    let (s, j) = fresh("b3").await;
    let (tx, mut rx) = mpsc::channel::<MarketEvent>(1);
    let out = BookOut::new(&tx, v("htx"), &s, &j);
    assert_eq!(out.book(&tx, book("htx", "X", 1)), Fed::Sent);
    core_out_book::invalidated(&tx, v("htx"), "A");
    assert_eq!(
        out.book(&tx, book("htx", "A", 2)),
        Fed::Kept,
        "ключ A ждёт — кадр слит при любой настройке"
    );
    assert_eq!(recv(&mut rx, 2).await, ["X@1", "inv A"]);
    // A@2 ждёт в наборе (доставщик стоит в очереди канала и получил освободившееся место) — это остаток срочного
    // слота, а не отложенное настройкой.
    assert_eq!(
        out.book(&tx, book("htx", "B", 1)),
        Fed::Lost,
        "настройка не задана — как раньше"
    );
    assert_eq!(recv(&mut rx, 1).await, ["A@2"]);
    tokio::task::yield_now().await;
    assert!(rx.try_recv().is_err(), "потерянный B не пришёл");
}

/// Ключ теста guard: один класс.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct G(u32);

impl Keyed for G {
    type Class = u8;
    fn class(&self) -> u8 {
        0
    }
}

/// Слот, который паникует при доставке, если велено.
struct Boom(u32, bool);

impl Slot for Boom {
    type Msg = u32;
    fn merge(&mut self, newer: Self) -> bool {
        *self = newer;
        true
    }
    fn now(self) -> u32 {
        self.0
    }
    fn take(self) -> Take<u32, Self> {
        assert!(!self.1, "паника доставщика (тест guard)");
        Take::Send(self.0, None)
    }
    fn urgent(&self) -> bool {
        false
    }
}

#[tokio::test]
async fn deliverer_guard_resets_on_panic_and_abort() {
    let (tx, mut rx) = mpsc::channel::<u32>(1);
    let set: Pending<G, Boom> = Pending::default();
    assert_eq!(set.offer(&tx, G(1), Boom(1, false), || true), Offer::Sent);
    let r = set.offer(&tx, G(2), Boom(2, true), || true);
    assert_eq!(r, Offer::Deferred { start: true });
    let worker = tokio::spawn(set.clone().deliver(tx.clone(), ()));
    assert_eq!(rx.recv().await, Some(1));
    assert!(worker.await.expect_err("паника").is_panic());
    // Признак «работает» сброшен — следующий отложенный снова запускает доставщика.
    assert_eq!(set.offer(&tx, G(3), Boom(3, false), || true), Offer::Sent);
    let r = set.offer(&tx, G(4), Boom(4, false), || true);
    assert_eq!(r, Offer::Deferred { start: true });
    // Снятие задачи, ждущей места в канале, — тоже сброс.
    let worker = tokio::spawn(set.clone().deliver(tx.clone(), ()));
    tokio::task::yield_now().await;
    worker.abort();
    assert!(worker.await.expect_err("снята").is_cancelled());
    let r = set.offer(&tx, G(5), Boom(5, false), || true);
    assert_eq!(r, Offer::Deferred { start: true });
    let worker = tokio::spawn(set.clone().deliver(tx.clone(), ()));
    assert_eq!(rx.recv().await, Some(3));
    assert_eq!(rx.recv().await, Some(4));
    assert_eq!(rx.recv().await, Some(5));
    assert!(worker.await.expect("доставщик"));
}
