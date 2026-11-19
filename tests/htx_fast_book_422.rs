//! Файл: tests/htx_fast_book_422.rs
//! Что делает: решение 422 — быстрый стакан HTX (канал /feed market.$symbol.mbp.$levels) на ПРИМЕРАХ ОФИЦИАЛЬНОЙ
//! ДОКУМЕНТАЦИИ (https://huobiapi.github.io/docs/spot/v1/en/, «Websocket Market Data → Market By Price (incremental
//! update)»): вид подписки и запроса снимка, разбор изменения и ответа на запрос, процедура синхронизации (снимок,
//! цепочка prevSeqNum → разрыв), выбор «быстрый стакан / снимок» (book_mode.rs) на стакане из примера ответа.
//! Живых кадров канала /feed в tests/data нет (api.huobi.pro закрыт прокси среды разработки) — что записать:
//! docs/TASKS.md, решение 422. Моков нет: кадры — дословно примеры документации.
//!
//! Кто вызывает: cargo test. От чего зависит: arb::venues::htx.

use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::types::{Asset, OrderBook, VenueId};
use arb::venues::htx::book::Books;
use arb::venues::htx::book_mode::{Back, Snap};
use arb::venues::htx::fast_book::{Applied, LocalBook, Update};
use arb::venues::htx::mbp::{self, Feed};

/// Пример документации: изменение («Example of incremental update»).
const UPDATE: &str = r#"{"ch":"market.btcusdt.mbp.5","ts":1573199608679,"tick":{"seqNum":100020146795,"prevSeqNum":100020146794,"asks":[[645.140000000000000000,26.755973959140651643]]}}"#;

/// Пример документации: ответ на запрос снимка («Refresh response»).
const REFRESH: &str = r#"{"id":"id2","rep":"market.btcusdt.mbp.5","status":"ok","data":{"seqNum":100020142010,"bids":[[618.37,71.594],[423.33,77.726],[223.18,47.997],[219.34,24.82],[210.34,94.463]],"asks":[[650.59,14.909733438479636],[650.63,97.996],[650.77,97.465],[651.23,83.973],[651.42,34.465]]}}"#;

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// Изменение из примера в числах ядра.
fn update() -> Update {
    let Feed::Update {
        symbol,
        seq,
        prev,
        bids,
        asks,
        ts,
    } = mbp::parse(UPDATE)
    else {
        panic!("изменение не разобрано")
    };
    assert_eq!(symbol, "btcusdt");
    Update {
        seq,
        prev,
        bids,
        asks,
        ts,
    }
}

/// Снимок из примера: (seqNum, bids, asks).
fn refresh() -> (
    i64,
    Vec<arb::core::types::PriceLevel>,
    Vec<arb::core::types::PriceLevel>,
) {
    let Feed::Refresh {
        symbol,
        seq,
        bids,
        asks,
    } = mbp::parse(REFRESH)
    else {
        panic!("ответ на запрос снимка не разобран")
    };
    assert_eq!(symbol, "btcusdt");
    (seq, bids, asks)
}

/// Подписка и запрос — вид из документации: {"sub": тема} и {"req": тема}, тема market.$symbol.mbp.$levels.
#[test]
fn messages_are_official_shape() {
    let sub: Value = serde_json::from_str(&mbp::sub_message("btcusdt", 5)).unwrap();
    assert_eq!(sub, json!({"sub": "market.btcusdt.mbp.5", "id": "btcusdt"}));
    let req: Value = serde_json::from_str(&mbp::req_message("btcusdt", 5)).unwrap();
    assert_eq!(
        req,
        json!({"req": "market.btcusdt.mbp.5", "id": "req.btcusdt"})
    );
}

/// Разбор примеров: у изменения 5 уровней — только изменившаяся сторона (asks), время кадра ts; у ответа — seqNum и
/// обе стороны по 5 уровней.
#[test]
fn docs_examples_parse() {
    let u = update();
    assert_eq!(
        (u.seq, u.prev, u.ts),
        (100020146795, 100020146794, Some(1573199608679))
    );
    assert!(u.bids.is_empty());
    assert_eq!(
        (u.asks[0].price, u.asks[0].qty),
        (d("645.14"), d("26.755973959140651643"))
    );
    let (seq, bids, asks) = refresh();
    assert_eq!((seq, bids.len(), asks.len()), (100020142010, 5, 5));
    assert_eq!((bids[0].price, asks[0].price), (d("618.37"), d("650.59")));
}

/// Процедура: до снимка изменения копятся; снимок даёт стакан; изменение, чей prevSeqNum не равен seqNum
/// предыдущего сообщения (в примерах документации 100020146794 ≠ 100020142010), — разрыв, стакан сброшен.
#[test]
fn sequence_rules_on_docs_examples() {
    let (seq, bids, asks) = refresh();
    let mut lb = LocalBook::new(5);
    let shot = lb.snapshot(seq, &bids, &asks);
    assert_eq!(shot, Applied::Changed);
    let (b, a) = lb.top(5);
    assert_eq!((b, a), (bids.clone(), asks.clone()));
    assert!(matches!(lb.apply(update()), Applied::Gap(_)));
    assert!(!lb.synced());
    // Отложенное до снимка изменение не продолжает снимок — тоже разрыв.
    let mut lb = LocalBook::new(5);
    assert_eq!(lb.apply(update()), Applied::Waiting);
    assert!(matches!(lb.snapshot(seq, &bids, &asks), Applied::Gap(_)));
}

/// Выбор book_mode.rs на стакане из примера: пара на снимках — снимок ядру; быстрый стакан со временем биржи —
/// пара на быстром; тот же снимок — подтверждение (время кадра пары); другой снимок, а быстрый не подтверждён дольше
/// срока, — расхождение; уход — запасной снимок.
#[test]
fn mode_switches_on_docs_book() {
    let (_, bids, asks) = refresh();
    let book = OrderBook {
        venue: VenueId("htx".into()),
        asset: Asset {
            ticker: "BTC".into(),
            bsc_contract: None,
        },
        bids: bids.clone(),
        asks: asks.clone(),
        exchange_time: None,
        received_at: chrono::Utc::now(),
    };
    let books = Books::default();
    let alive = Arc::new(AtomicI64::new(0));
    books.mode.set_levels(5);
    let never = || None;
    assert_eq!(
        books
            .mode
            .on_snapshot(&books, "BTC", Some(&book), &alive, 20, never),
        Snap::Give
    );
    books.put(book.clone(), alive.clone());
    assert!(books.mode.on_fast("BTC", update().ts));
    assert_eq!(
        books
            .mode
            .on_snapshot(&books, "BTC", Some(&book), &alive, 20, never),
        Snap::Touch
    );
    let mut other = book.clone();
    other.asks.truncate(1);
    assert_eq!(
        books
            .mode
            .on_snapshot(&books, "BTC", Some(&other), &alive, 20, never),
        Snap::Hold
    );
    assert_eq!(
        books
            .mode
            .on_snapshot(&books, "BTC", Some(&other), &alive, 20, || Some(-1)),
        Snap::Diverged
    );
    assert!(matches!(books.mode.fall_back("BTC", "тест"), Back::Use(b, _) if b.asks.len() == 1));
    assert!(matches!(books.mode.fall_back("BTC", "тест"), Back::Nothing));
}
