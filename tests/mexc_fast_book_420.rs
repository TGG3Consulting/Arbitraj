//! Файл: tests/mexc_fast_book_420.rs
//! Что делает: решение 420 — быстрый стакан MEXC (venues/mexc/fast_book.rs, depth.rs Msg::Diff, fast_snap.rs parse) на
//! примерах из официальной документации MEXC (живых кадров канала изменений в tests/data нет — CLAUDE.md: тесты только
//! на реальных данных, синтетических последовательностей версий нет):
//! - Diff.Depth Stream (https://www.mexc.com/api-docs/spot-v3/websocket-market-streams/diffdepth-stream), пример ответа:
//!   channel «spot@public.aggre.depth.v3.api.pb@100ms@BTCUSDT», symbol «BTCUSDT», sendTime 1778075380657,
//!   publicAggreDepths {asks: [], bids: [{price "100", quantity "4"}], eventType «spot@public.aggre.depth.v3.api.pb@100ms»,
//!   fromVersion "10023", toVersion "10023"} — собран в protobuf по полям proto.rs (тот же пример, другой вид записи);
//! - Order Book (https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/order-book), пример ответа:
//!   {"lastUpdateId":1112416,"bids":[["15.00000","49999.00000"]],"asks":[["14.0000","1.0000"]]}.
//!
//! Проверки последовательности на живых кадрах (строгое fromVersion = прежний toVersion + 1, разрыв, шаг 4) — тест
//! #[ignore] на записи окна tests/data/mexc_ws/depth_diff.hex + depth_snapshot.json (как записать — tests/README.md).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::mexc::{fast_book, fast_snap, depth, proto}, prost, rust_decimal.

use std::str::FromStr;

use arb::venues::mexc::depth::{Msg, parse_binary};
use arb::venues::mexc::fast_book::{Applied, LocalBook};
use arb::venues::mexc::fast_snap::parse;
use arb::venues::mexc::proto::{AggreDepths, Body, DepthItem, Wrapper};
use prost::Message as _;
use rust_decimal::Decimal;

/// Кадр примера Diff.Depth Stream в protobuf. Ошибок нет.
fn doc_frame() -> Vec<u8> {
    Wrapper {
        channel: "spot@public.aggre.depth.v3.api.pb@100ms@BTCUSDT".into(),
        body: Some(Body::AggreDepths(AggreDepths {
            asks: vec![],
            bids: vec![DepthItem {
                price: "100".into(),
                quantity: "4".into(),
            }],
            event_type: "spot@public.aggre.depth.v3.api.pb@100ms".into(),
            from_version: "10023".into(),
            to_version: "10023".into(),
        })),
        symbol: Some("BTCUSDT".into()),
        create_time: None,
        send_time: Some(1778075380657),
    }
    .encode_to_vec()
}

/// Ответ примера Order Book. Ошибок нет.
fn doc_snapshot() -> serde_json::Value {
    serde_json::json!({"lastUpdateId":1112416,"bids":[["15.00000","49999.00000"]],"asks":[["14.0000","1.0000"]]})
}

/// Число из строки. Ошибок нет (паника на нечисле).
fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Кадр примера разбирается в Msg::Diff: символ, версии, уровень, sendTime.
#[test]
fn doc_diff_frame_parses() {
    let Msg::Diff { symbol, frame } = parse_binary(&doc_frame()) else {
        panic!("кадр изменений")
    };
    let u = frame.unwrap();
    assert_eq!(symbol, "BTCUSDT");
    assert_eq!((u.from, u.to), (10023, 10023));
    assert_eq!(u.bids.len(), 1);
    assert_eq!((u.bids[0].price, u.bids[0].qty), (d("100"), d("4")));
    assert!(u.asks.is_empty());
    assert_eq!(u.time_ms, Some(1778075380657));
}

/// Снимок примера: lastUpdateId и уровни.
#[test]
fn doc_snapshot_parses() {
    let s = parse(&doc_snapshot()).unwrap();
    assert_eq!(s.id, 1112416);
    assert_eq!(
        (s.bids[0].price, s.bids[0].qty),
        (d("15.00000"), d("49999.00000"))
    );
    assert_eq!(
        (s.asks[0].price, s.asks[0].qty),
        (d("14.0000"), d("1.0000"))
    );
}

/// Шаги 2, 3, 5, 7: запрос снимка ушёл (start_buffer) — кадр до снимка отложен; снимок новее кадра — кадр отброшен (toVersion ≤ lastUpdateId), стакан —
/// снимок, версия синхронизации есть, времени кадра нет (ядру — с первым кадром после снимка); тот же кадр после
/// снимка — уже учтён.
#[test]
fn doc_frame_then_snapshot() {
    let Msg::Diff { frame, .. } = parse_binary(&doc_frame()) else {
        panic!("кадр изменений")
    };
    let u = frame.unwrap();
    let mut lb = LocalBook::default();
    lb.start_buffer();
    assert_eq!(lb.apply(u.clone()), Applied::Waiting);
    assert!(!lb.synced());
    let s = parse(&doc_snapshot()).unwrap();
    assert_eq!(lb.snapshot(s.id, &s.bids, &s.asks), Applied::Changed);
    assert!(lb.synced());
    assert_eq!(lb.time_ms(), None);
    let (b, a) = lb.top(20);
    assert_eq!((b.len(), a.len()), (1, 1));
    assert_eq!(b[0].price, d("15.00000"));
    assert_eq!(lb.apply(u), Applied::Repeat);
}

/// Строки hex файла. Ошибок нет (паника с причиной).
fn hex_lines(path: &str) -> Vec<Vec<u8>> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| hex::decode(l.trim()).unwrap())
        .collect()
}

/// Живая запись окна: снимок потока Snapshot и кадры потока Diff подряд — после снимка каждый применённый кадр строго
/// продолжает версию (иначе Gap с причиной — тест это показывает), стакан синхронизирован.
#[test]
#[ignore = "нет живой записи канала изменений MEXC — tests/README.md (4)"]
fn recorded_diff_sequence() {
    let snap: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string("tests/data/mexc_ws/depth_snapshot.json").unwrap(),
    )
    .unwrap();
    let s = parse(&snap).unwrap();
    let mut lb = LocalBook::default();
    assert_eq!(lb.snapshot(s.id, &s.bids, &s.asks), Applied::Changed);
    for b in hex_lines("tests/data/mexc_ws/depth_diff.hex") {
        let Msg::Diff { frame, .. } = parse_binary(&b) else {
            continue;
        };
        let r = lb.apply(frame.unwrap());
        assert!(!matches!(r, Applied::Gap(_)), "{r:?}");
    }
    assert!(lb.synced());
}
