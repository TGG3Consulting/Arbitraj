//! Файл: tests/gateway_book_events_keep_324_review.rs
//! Что делает: ревью решения 324 — общий набор канала ядра на настоящем канале tokio и настоящем хранилище настроек
//! (SQLite во временной папке); данных площадок нет: стаканы — пустые книги с разным временем получения, событие пула —
//! метка (MarketEvent::Notice с именем пула).
//! - Б1 быстрый путь: набор пуст — Fast::Free / Fast::Full без замка, ключа и настройки (настройка не читается при
//!   свободном канале); кто-то ждёт — Fast::Busy (только медленный путь); набор опустел — снова Free;
//! - Н5: канал закрыт, а ключ пары ждёт — кадр сливается с ждущим (Fed::Kept), доставщик очищает набор, дальше Closed;
//! - Live площадки DEX в наборе (Feed::live, проверка вне замка — Н4): «не годный» пул не доставляется;
//! - параллельные производители одного ключа (многопоточный рантайм): события КАЖДОГО производителя приходят в его
//!   порядке (гарантия порядка keep_last.rs для упорядоченных предложений); порядок между производителями не задан
//!   и не проверяется. Результат не зависит от таймингов — проверяется свойство любого чередования.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::gateway::{core_out, core_out_book, core_out_item, keep_last}, arb::settings, arb::storage, tokio.
//! Настройки: venue.book_events_keep (задаётся тестом в своей временной базе).

use std::cell::Cell;
use std::sync::Arc;

use alloy_primitives::Address;
use arb::core::types::{Asset, OrderBook, PoolRef, VenueId};
use arb::gateway::core_out;
use arb::gateway::core_out_book::{BookOut, Fed};
use arb::gateway::core_out_item::{Class, Item, Key};
use arb::gateway::events::MarketEvent;
use arb::gateway::keep_last::{Fast, Keyed, Offer, Pending, Slot, Take};
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use serde_json::json;
use tokio::sync::{broadcast, mpsc};

/// Новая база во временной папке: настройки и журнал. Ошибок нет (паника теста).
async fn fresh(name: &str) -> (Store, Journal) {
    let dir = std::env::temp_dir().join(format!("arb-t324r-{name}-{}", std::process::id()));
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

/// Метка события: «A@2» — кадр, имя пула — пул.
fn tag(ev: &MarketEvent) -> String {
    match ev {
        MarketEvent::Book(b) => format!("{}@{}", b.asset.ticker, b.received_at.timestamp_millis()),
        MarketEvent::Notice { text, .. } => text.clone(),
        _ => "?".into(),
    }
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
async fn fast_path_only_while_set_empty() {
    let (tx, mut rx) = mpsc::channel::<MarketEvent>(1);
    let feed = core_out::of(&tx);
    assert!(
        matches!(feed.fast(&tx), Fast::Free(_)),
        "пусто — место есть"
    );
    let read = Cell::new(0);
    let keep = || {
        read.set(read.get() + 1);
        true
    };
    let (k, i) = pool(1);
    assert_eq!(feed.offer(&tx, k, i, keep), Offer::Sent);
    assert_eq!(read.get(), 0, "при свободном канале настройка не читается");
    assert!(
        matches!(feed.fast(&tx), Fast::Full),
        "набор пуст, канал полон"
    );
    let (k, i) = pool(2);
    let r = feed.offer(&tx, k, i, || true);
    assert_eq!(r, Offer::Deferred { start: true });
    assert!(
        matches!(feed.fast(&tx), Fast::Busy),
        "кто-то ждёт — только медленный путь"
    );
    assert_eq!(tag(&rx.recv().await.expect("P1")), "P1");
    assert_eq!(tag(&rx.recv().await.expect("P2")), "P2");
    // Доставщик отправил P2 и опубликовал busy = 0 до того, как отдал управление.
    assert_eq!(feed.waiting(), 0);
    assert!(
        matches!(feed.fast(&tx), Fast::Free(_)),
        "набор опустел — снова быстрый путь"
    );
}

#[tokio::test]
async fn closed_channel_with_waiting_pair_is_kept_then_closed() {
    let (s, j) = fresh("closed").await;
    s.set(
        "venue.book_events_keep",
        Scope::Venue("phemex".into()),
        Some(json!(true)),
        "тест",
    )
    .await
    .expect("настройка");
    let (tx, rx) = mpsc::channel::<MarketEvent>(1);
    let out = BookOut::new(&tx, v("phemex"), &s, &j);
    assert_eq!(out.book(&tx, book("phemex", "A", 1)), Fed::Sent);
    assert_eq!(out.book(&tx, book("phemex", "A", 2)), Fed::Kept);
    drop(rx);
    // Н5: ключ пары ждёт — кадр сливается с ждущим, хотя канал уже закрыт (Kept, а не Closed).
    assert_eq!(out.book(&tx, book("phemex", "A", 3)), Fed::Kept);
    // Доставщик видит закрытый канал и очищает набор.
    tokio::task::yield_now().await;
    assert_eq!(core_out::of(&tx).waiting(), 0, "мёртвое не держим");
    assert_eq!(out.book(&tx, book("phemex", "A", 4)), Fed::Closed);
}

#[tokio::test]
async fn dead_pool_not_delivered_by_venue_live() {
    let (tx, mut rx) = mpsc::channel::<MarketEvent>(1);
    let feed = core_out::of(&tx);
    // Пул P2 «не годен» (как снятый / недостоверный в реестре DEX).
    let live = |ev: &MarketEvent| !matches!(ev, MarketEvent::Notice { text, .. } if text == "P2");
    feed.live(v("bsc_dex"), Arc::new(live));
    for n in 1..=3 {
        let (k, i) = pool(n);
        let _ = feed.offer(&tx, k, i, || true);
    }
    assert_eq!(tag(&rx.recv().await.expect("P1")), "P1");
    assert_eq!(tag(&rx.recv().await.expect("P3")), "P3");
    assert_eq!(feed.waiting(), 0);
    let c = feed.take(&Class::Pool(v("bsc_dex")));
    assert_eq!((c.delivered, c.dropped), (1, 1));
}

/// Ключ теста производителей: один.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct One;

impl Keyed for One {
    type Class = u8;
    fn class(&self) -> u8 {
        0
    }
}

/// Событие производителя: (производитель, номер).
struct Seq(u32, u32);

impl Slot for Seq {
    type Msg = (u32, u32);
    fn merge(&mut self, newer: Self) -> bool {
        *self = newer;
        true
    }
    fn now(self) -> (u32, u32) {
        (self.0, self.1)
    }
    fn take(self) -> Take<(u32, u32), Self> {
        Take::Send((self.0, self.1), None)
    }
    fn urgent(&self) -> bool {
        false
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_producers_of_one_key_keep_own_order() {
    let (tx, mut rx) = mpsc::channel::<(u32, u32)>(2);
    let set: Pending<One, Seq> = Pending::default();
    let mut tasks = vec![];
    for p in 0..2u32 {
        let (set, tx) = (set.clone(), tx.clone());
        tasks.push(tokio::spawn(async move {
            for n in 0..2000u32 {
                let r = set.offer(&tx, One, Seq(p, n), || true);
                if matches!(
                    r,
                    Offer::Deferred { start: true } | Offer::Replaced { start: true }
                ) {
                    tokio::spawn(set.clone().deliver(tx.clone(), ()));
                }
                if n % 64 == 0 {
                    tokio::task::yield_now().await;
                }
            }
        }));
    }
    drop(tx);
    let reader = tokio::spawn(async move {
        let mut got = vec![];
        while let Some(m) = rx.recv().await {
            got.push(m);
            if got.len() % 7 == 0 {
                tokio::task::yield_now().await;
            }
        }
        got
    });
    for t in tasks {
        t.await.expect("производитель");
    }
    // Все Sender сняты (доставщик снимает свой, когда набор пуст) — читатель кончается.
    let got = reader.await.expect("читатель");
    for p in 0..2u32 {
        let own: Vec<u32> = got.iter().filter(|m| m.0 == p).map(|m| m.1).collect();
        assert!(
            own.windows(2).all(|w| w[0] < w[1]),
            "производитель {p}: события пришли не по порядку"
        );
    }
}
