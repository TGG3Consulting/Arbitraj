//! Файл: tests/phemex_book_pick_427.rs
//! Что делает: решение 427 (Phemex, запасной поток стакана) на кадрах ИЗ ОФИЦИАЛЬНОЙ ДОКУМЕНТАЦИИ (Spot Websocket API →
//! OrderBook message, https://phemex-docs.github.io/#orderbook-message): «Message sample: snapshot» (sBTCUSDT, sequence
//! 677996311, timestamp 1590570810187571000) и «Message sample: incremental update» (sequence 677996941, timestamp
//! 1590570811244189000, bids [[892387000000, 4792900], [892226000000, 0]]). Живых кадров двух соединений нет
//! (api.phemex.com закрыт прокси среды разработки) — пункт «записать живые кадры» в docs/TASKS.md. Моков нет: стакан
//! строит настоящий local_book.rs, выбор — настоящий book_pick.rs.
//! - контрольный снимок с тем же sequence сверяется со стаканом в памяти (local_book.rs Applied::Differs);
//! - выбор потока: основной главный; потеря основного — запасной; стакан старее отданного ядру не отдаётся; без
//!   запасного потока — потеря = «недостоверно».
//!
//! Кто вызывает: cargo test. От чего зависит: arb::venues::phemex (depth, local_book, book_pick), arb::core::types.

use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use arb::core::types::{Asset, OrderBook, VenueId};
use arb::venues::phemex::book_pick::{Event, Out, Pick, Role};
use arb::venues::phemex::{depth, local_book};

/// «Message sample: snapshot» документации.
const SNAP: &str = r#"{"book":{"asks":[[892697000000,1781800],[892708000000,7543500]],"bids":[[892376000000,6866500],[892354000000,14209000]]},"depth":30,"sequence":677996311,"symbol":"sBTCUSDT","timestamp":1590570810187571000,"type":"snapshot"}"#;
/// «Message sample: incremental update» документации.
const INC: &str = r#"{"book":{"asks":[],"bids":[[892387000000,4792900],[892226000000,0]]},"depth":30,"sequence":677996941,"symbol":"sBTCUSDT","timestamp":1590570811244189000,"type":"incremental"}"#;
const T_SNAP: i64 = 1590570810187571000;
const T_INC: i64 = 1590570811244189000;

/// Кадр стакана из текста. Паника — кадр не разобран.
fn frame(text: &str) -> depth::Frame {
    match depth::parse(text) {
        depth::Msg::Book(f) => f,
        other => panic!("кадр OrderBook не разобран: {other:?}"),
    }
}

/// Стакан ядра из локального стакана (масштабы 8 / 8, как в tests/phemex_real_data.rs).
fn book(lb: &local_book::LocalBook) -> OrderBook {
    let (bids, asks) = lb.top(30, 8, 8);
    OrderBook {
        venue: VenueId("phemex".into()),
        asset: Asset {
            ticker: "BTC".into(),
            chain: None,
            contract: None,
        },
        bids,
        asks,
        exchange_time: None,
        received_at: chrono::Utc::now(),
    }
}

#[test]
fn snapshot_same_sequence_is_verified() {
    let mut lb = local_book::LocalBook::default();
    assert_eq!(lb.apply(&frame(SNAP), true), local_book::Applied::Changed);
    // Тот же снимок ещё раз (тот же sequence, те же уровни) — совпадает.
    assert_eq!(lb.apply(&frame(SNAP), true), local_book::Applied::Changed);
    // Изменение документации, затем снимок с ЕГО sequence, но уровнями прежнего снимка — расхождение.
    assert_eq!(lb.apply(&frame(INC), false), local_book::Applied::Changed);
    let stale = SNAP.replace("677996311", "677996941");
    assert_eq!(
        lb.apply(&frame(&stale), false),
        local_book::Applied::Differs
    );
    // Снимок с другим sequence не сверяется (связь номеров не описана).
    let other = SNAP.replace("677996311", "677997000");
    assert_eq!(
        lb.apply(&frame(&other), false),
        local_book::Applied::Changed
    );
}

#[test]
fn reserve_takes_over_and_never_older() {
    let alive = Arc::new(AtomicI64::new(0));
    let pick = Pick::new(&["BTC".to_string()], true);
    let mut main = local_book::LocalBook::default();
    let mut res = local_book::LocalBook::default();
    main.apply(&frame(SNAP), false);
    res.apply(&frame(SNAP), false);
    let (g, _) = pick.frame(Role::Main, "BTC", book(&main), Some(T_SNAP), &alive, 1);
    assert!(g.is_some(), "основной синхронизирован — главный");
    let (g, _) = pick.frame(Role::Reserve, "BTC", book(&res), Some(T_SNAP), &alive, 2);
    assert!(g.is_none(), "запасной при живом основном не отдаётся");
    // Основной потерян: последний кадр запасного не новее отданного — стакан в памяти остаётся.
    let (out, ev) = pick.lost(Role::Main, "BTC");
    assert!(matches!(out, Out::Keep(_)));
    assert_eq!(ev, vec![Event::MainGap, Event::ToReserve]);
    // Новое изменение запасного — ядру.
    res.apply(&frame(INC), false);
    let (g, _) = pick.frame(Role::Reserve, "BTC", book(&res), Some(T_INC), &alive, 3);
    let g = g.expect("запасной служит");
    assert_eq!(
        g.bids[0].price,
        "8923.87".parse::<rust_decimal::Decimal>().unwrap()
    );
    // Основной снова синхронизирован старым снимком — главный, но кадр старее отданного ядру не уходит.
    let (g, ev) = pick.frame(Role::Main, "BTC", book(&main), Some(T_SNAP), &alive, 4);
    assert!(g.is_none());
    assert_eq!(ev, vec![Event::ToMain]);
    let (g, _) = pick.frame(Role::Reserve, "BTC", book(&res), Some(T_INC), &alive, 5);
    assert!(g.is_none(), "основной снова главный");
}

#[test]
fn reserve_newer_book_given_at_once() {
    let alive = Arc::new(AtomicI64::new(0));
    let pick = Pick::new(&["BTC".to_string()], true);
    let mut main = local_book::LocalBook::default();
    let mut res = local_book::LocalBook::default();
    main.apply(&frame(SNAP), false);
    res.apply(&frame(SNAP), false);
    res.apply(&frame(INC), false);
    pick.frame(Role::Main, "BTC", book(&main), Some(T_SNAP), &alive, 1);
    pick.frame(Role::Reserve, "BTC", book(&res), Some(T_INC), &alive, 2);
    match pick.lost(Role::Main, "BTC").0 {
        Out::Give(t) => {
            assert_eq!(t.seen_ms, 2, "время кадра запасного потока");
            assert_eq!(
                t.book.bids[0].price,
                "8923.87".parse::<rust_decimal::Decimal>().unwrap()
            );
        }
        _ => panic!("ожидался стакан запасного потока"),
    }
    // Потерян и запасной — стакан недостоверен.
    let (out, ev) = pick.lost(Role::Reserve, "BTC");
    assert!(matches!(out, Out::Invalidate));
    assert_eq!(ev, vec![Event::ReserveGap, Event::Blind]);
}

#[test]
fn without_reserve_gap_invalidates() {
    let alive = Arc::new(AtomicI64::new(0));
    let pick = Pick::new(&["BTC".to_string()], false);
    let mut main = local_book::LocalBook::default();
    main.apply(&frame(SNAP), false);
    assert!(
        pick.frame(Role::Main, "BTC", book(&main), None, &alive, 1)
            .0
            .is_some()
    );
    let (out, ev) = pick.lost(Role::Main, "BTC");
    assert!(matches!(out, Out::Invalidate));
    assert_eq!(ev, vec![Event::MainGap]);
    let (g, ev) = pick.frame(Role::Main, "BTC", book(&main), None, &alive, 2);
    assert!(g.is_some());
    assert_eq!(ev, vec![Event::MainResync]);
}

/// Ревью 427: контрольный снимок с тем же sequence (и тем же timestamp), исправивший стакан в памяти (Differs), доходит
/// до ядра; deliver (память и канал ядра) вызывается только для отданного стакана.
#[test]
fn same_timestamp_snapshot_reaches_core() {
    let alive = Arc::new(AtomicI64::new(0));
    let pick = Pick::new(&["BTC".to_string()], true);
    let mut main = local_book::LocalBook::default();
    main.apply(&frame(SNAP), false);
    main.apply(&frame(INC), false);
    let (g, _) = pick.frame(Role::Main, "BTC", book(&main), Some(T_INC), &alive, 1);
    assert!(g.is_some());
    let stale = SNAP
        .replace("677996311", "677996941")
        .replace("1590570810187571000", "1590570811244189000");
    assert_eq!(
        main.apply(&frame(&stale), false),
        local_book::Applied::Differs
    );
    let (n, _) = pick.frame_with(
        Role::Main,
        "BTC",
        book(&main),
        Some(T_INC),
        &alive,
        2,
        |b| b.bids.len(),
    );
    assert_eq!(n, Some(2), "исправленный снимок с тем же timestamp отдан");
    let mut called = false;
    let (n, _) = pick.frame_with(
        Role::Main,
        "BTC",
        book(&main),
        Some(T_SNAP),
        &alive,
        3,
        |_| {
            called = true;
        },
    );
    assert!(n.is_none() && !called, "кадр старее отданного не отдаётся");
}
