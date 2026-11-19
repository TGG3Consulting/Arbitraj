//! Файл: tests/binance_sbe_book.rs
//! Что делает: решение 425 — локальный стакан Binance (venues/binance/sbe_book.rs) по процедуре «How to manage a local
//! order book correctly» на ПРИМЕРАХ ИЗ ОФИЦИАЛЬНОЙ ДОКУМЕНТАЦИИ (github.com/binance/binance-spot-api-docs):
//! - изменения из примера «Diff. Depth Stream» (web-socket-streams.md: "U": 157, "u": 160, "b": [["0.0024","10"]],
//!   "a": [["0.0026","100"]]) до запроса снимка не нужны (Idle), во время запроса — откладываются;
//! - снимок из примера «Order book» (rest-api.md: "lastUpdateId": 1027024, bids [["4.00000000","431.00000000"]], asks
//!   [["4.00000200","12.00000000"]]): lastUpdateId не меньше U отложенного (пункт 4 — снимок годен), отложенное с
//!   u ≤ lastUpdateId отброшено (пункт 5), стакан = снимок; тот же кадр после снимка — Old.
//!
//! Живых кадров SBE (stream-sbe.binance.com) нет — из среды разработки Binance закрыт прокси; двоичных примеров в
//! документации нет, поэтому декодер sbe.rs тестами на кадрах не покрыт: запись живых кадров — docs/TASKS.md
//! (решение 425). Синтетических последовательностей нет. Моков нет.
//! Кто вызывает: cargo test. От чего зависит: arb::venues::binance::sbe_book, arb::core::types.

use std::str::FromStr;

use rust_decimal::Decimal;

use arb::core::types::PriceLevel;
use arb::venues::binance::sbe_book::{Applied, LocalBook, Update};

/// Уровень из строк документации.
fn lv(p: &str, q: &str) -> PriceLevel {
    PriceLevel {
        price: Decimal::from_str(p).unwrap(),
        qty: Decimal::from_str(q).unwrap(),
    }
}

/// Изменения из примера «Diff. Depth Stream».
fn diff_doc() -> Update {
    Update {
        first: 157,
        last: 160,
        bids: vec![lv("0.0024", "10")],
        asks: vec![lv("0.0026", "100")],
    }
}

#[test]
fn before_snapshot_request_event_is_not_needed() {
    let mut b = LocalBook::default();
    assert_eq!(b.apply(diff_doc()), Applied::Idle);
    assert!(!b.synced());
}

#[test]
fn doc_snapshot_discards_older_buffered_event() {
    let mut b = LocalBook::default();
    b.begin();
    assert_eq!(b.apply(diff_doc()), Applied::Buffered);
    let bids = [lv("4.00000000", "431.00000000")];
    let asks = [lv("4.00000200", "12.00000000")];
    assert_eq!(b.snapshot(1027024, &bids, &asks), Applied::Changed);
    assert_eq!(b.version(), Some(1027024));
    assert_eq!(b.top(20), (bids.to_vec(), asks.to_vec()));
    // Пункт «u меньше номера стакана — пропустить»: тот же кадр после снимка ничего не меняет.
    assert_eq!(b.apply(diff_doc()), Applied::Old);
    assert_eq!(b.top(20), (bids.to_vec(), asks.to_vec()));
}
