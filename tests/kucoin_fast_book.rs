//! Файл: tests/kucoin_fast_book.rs
//! Что делает: решение 423 — быстрый стакан KuCoin на реальных данных:
//! - правила калибровки local_book.rs — на примере официальной документации Orderbook - Increment
//!   (https://www.kucoin.com/docs-new/3470068w0: снимок sequence 16, кадр sequenceStart 15 — sequenceEnd 19, итог
//!   asks 3988.62×8, 3988.60×47, 3988.59×3; bids 3988.51×56, 3988.50×44, 3988.49×100, 3988.48×10), и с кадром до снимка
//!   (кэш и проигрывание — шаги 1–3 процедуры); ревью 423 (буфер): до ухода запроса снимка кадры не копятся;
//! - разбор кадров /market/level2 (incr.rs) и непрерывность номеров — на живой записи 04.11
//!   (tests/data/cex_samples_2026_11_04/kucoin_ws.jsonl, CAKE-USDT, sequence 783409212–783409229);
//! - разрыв: тот же поток без одного живого кадра — Gap;
//! - сверка fast_check.rs: стакан, начатый с первого живого кадра level2Depth50 (его timestamp равен time кадра
//!   изменений 783409212 — факт записи), после живых изменений совпадает (5 уровней) со всеми 10 следующими живыми
//!   кадрами level2Depth50; отпечаток другого времени — Diverge, когда стакан уходит дальше; ревью: два живых кадра
//!   level2Depth50 подряд новее застывшего стакана и не равны ему — Diverge;
//! - разбор ответа снимка (full_book.rs parse) — на живом ответе Get Part OrderBook level2_20 (те же поля time,
//!   sequence-строка, bids, asks; живой записи Get Full OrderBook нет — docs/TASKS.md).
//!
//! Кто вызывает: cargo test. От чего зависит: arb::venues::kucoin. Моков нет.

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::PriceLevel;
use arb::gateway::feed_health::BookPrint;
use arb::venues::kucoin::fast_check::{Check, Verdict};
use arb::venues::kucoin::local_book::{Applied, Change, LocalBook, Update};
use arb::venues::kucoin::{depth, full_book, incr};

const WS: &str = "tests/data/cex_samples_2026_11_04/kucoin_ws.jsonl";

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

fn lv(p: &str, q: &str) -> PriceLevel {
    PriceLevel {
        price: d(p),
        qty: d(q),
    }
}

fn ch(p: &str, q: &str, seq: i64) -> Change {
    Change {
        price: d(p),
        size: d(q),
        seq,
    }
}

/// Снимок и кадр примера документации. time в примере не дан и в правилах номеров не участвует.
fn doc_example() -> (Vec<PriceLevel>, Vec<PriceLevel>, Update) {
    let asks = vec![
        lv("3988.62", "8"),
        lv("3988.61", "32"),
        lv("3988.60", "47"),
        lv("3988.59", "3"),
    ];
    let bids = vec![
        lv("3988.51", "56"),
        lv("3988.50", "15"),
        lv("3988.49", "100"),
        lv("3988.48", "10"),
    ];
    let u = Update {
        start: 15,
        end: 19,
        time: 0,
        asks: vec![
            ch("3988.59", "3", 16),
            ch("3988.61", "0", 19),
            ch("3988.62", "8", 15),
        ],
        bids: vec![ch("3988.50", "44", 18)],
    };
    (bids, asks, u)
}

fn doc_result(b: &LocalBook) {
    let (bids, asks) = b.top(10);
    assert_eq!(
        asks,
        vec![lv("3988.59", "3"), lv("3988.60", "47"), lv("3988.62", "8")]
    );
    assert_eq!(
        bids,
        vec![
            lv("3988.51", "56"),
            lv("3988.50", "44"),
            lv("3988.49", "100"),
            lv("3988.48", "10")
        ]
    );
    assert_eq!(b.version(), Some(19));
}

#[test]
fn official_calibration_example() {
    let (bids, asks, u) = doc_example();
    let mut b = LocalBook::default();
    assert_eq!(b.snapshot(16, &bids, &asks), Applied::Changed);
    assert_eq!(b.apply(u), Applied::Changed);
    doc_result(&b);
}

#[test]
fn official_example_cached_before_snapshot() {
    let (bids, asks, u) = doc_example();
    let mut b = LocalBook::default();
    // Ревью 423 (буфер): кадры копятся с ухода запроса снимка из очереди.
    b.start();
    assert_eq!(b.apply(u), Applied::Waiting);
    assert!(!b.synced());
    assert_eq!(b.snapshot(16, &bids, &asks), Applied::Changed);
    doc_result(&b);
}

/// Ревью 423 (буфер): пока запрос снимка ждёт очереди (start не было), кадр примера документации не копится — после
/// снимка стакан равен снимку (номер 16), буфер не растёт.
#[test]
fn frames_before_request_left_queue_are_not_buffered() {
    let (bids, asks, u) = doc_example();
    let mut b = LocalBook::default();
    assert_eq!(b.apply(u), Applied::Waiting);
    assert_eq!(b.snapshot(16, &bids, &asks), Applied::Changed);
    assert_eq!(b.version(), Some(16));
    assert_eq!(b.top(10), (bids, asks.into_iter().rev().collect()));
}

/// Живые кадры сообщений (type = message) записи по порядку.
fn frames() -> Vec<Value> {
    std::fs::read_to_string(WS)
        .unwrap()
        .lines()
        .filter_map(|l| {
            let o: Value = serde_json::from_str(l).unwrap();
            let m: Value = serde_json::from_str(o.get("msg")?.as_str()?).unwrap();
            (m["type"] == "message").then_some(m)
        })
        .collect()
}

/// Живые изменения записи по порядку.
fn updates() -> Vec<Update> {
    frames()
        .iter()
        .filter(|m| m["topic"] == "/market/level2:CAKE-USDT")
        .map(|m| incr::parse(&m["data"]).unwrap())
        .map(|(sym, u)| {
            assert_eq!(sym, "CAKE-USDT");
            u
        })
        .collect()
}

/// Стакан, начатый с первого живого кадра level2Depth50 как со снимком номера 783409212 и временем этого кадра.
fn seeded() -> (LocalBook, usize) {
    let fr = frames();
    let i = fr
        .iter()
        .position(|m| m["topic"] == "/spotMarket/level2Depth50:CAKE-USDT")
        .unwrap();
    let depth::Msg::Depth { bids, asks, .. } = depth::parse(&fr[i].to_string()) else {
        panic!("снимок")
    };
    let mut b = LocalBook::default();
    b.snapshot(783409212, &bids, &asks);
    (b, i)
}

#[test]
fn live_increments_are_continuous() {
    let u = updates();
    assert_eq!(u.len(), 18);
    assert_eq!(
        (u[0].start, u[0].end, u[0].time),
        (783409212, 783409212, 1793740790630)
    );
    assert_eq!(u[0].bids, vec![ch("0.001", "15300", 783409212)]);
    let (mut b, _) = seeded();
    for x in u.into_iter().skip(1) {
        assert_eq!(b.apply(x), Applied::Changed);
    }
    assert_eq!(b.version(), Some(783409229));
    assert_eq!(b.time(), Some(1793740801546));
}

#[test]
fn missing_live_frame_is_a_gap() {
    let (mut b, _) = seeded();
    let mut gap = None;
    for x in updates()
        .into_iter()
        .skip(1)
        .filter(|x| x.start != 783409220)
    {
        if let Applied::Gap(why) = b.apply(x) {
            gap = Some(why);
            break;
        }
    }
    assert!(gap.unwrap().contains("783409221"));
    assert!(!b.synced());
}

#[test]
fn live_level50_matches_fast_book() {
    let fr = frames();
    let (mut b, first) = seeded();
    let mut c = Check::default();
    let mut ok = 0;
    for m in &fr[first + 1..] {
        if m["topic"] == "/market/level2:CAKE-USDT" {
            let (_, u) = incr::parse(&m["data"]).unwrap();
            assert_eq!(b.apply(u), Applied::Changed);
            let v = c.on_local(b.time().unwrap(), b.print(5));
            assert!(!matches!(v, Verdict::Diverge(_)), "{v:?}");
            continue;
        }
        let depth::Msg::Depth {
            bids,
            asks,
            exchange_time,
            ..
        } = depth::parse(&m.to_string())
        else {
            panic!()
        };
        let at = exchange_time.unwrap().timestamp_millis();
        assert_eq!(
            c.on_snapshot(at, BookPrint::top(&bids, &asks, 5), b.time()),
            Verdict::Ok,
            "{at}"
        );
        ok += 1;
    }
    assert_eq!(ok, 10);
}

/// Живые кадры level2Depth50 записи: (timestamp, отпечаток верхних 5 уровней).
fn snapshots() -> Vec<(i64, BookPrint)> {
    frames()
        .iter()
        .filter_map(|m| match depth::parse(&m.to_string()) {
            depth::Msg::Depth {
                bids,
                asks,
                exchange_time,
                ..
            } => Some((
                exchange_time?.timestamp_millis(),
                BookPrint::top(&bids, &asks, 5),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn unmatched_snapshot_diverges_when_book_moves_on() {
    let snaps = snapshots();
    // Снимок 1793740793775 (bid 1.892 × 377.2251) против состояния того же времени с отпечатком прежнего снимка.
    let (at, want) = snaps[4].clone();
    let mut c = Check::default();
    assert_eq!(c.on_local(at, snaps[3].1.clone()), Verdict::Skip);
    assert_eq!(c.on_snapshot(at, want, Some(at)), Verdict::Wait);
    assert!(matches!(
        c.on_local(snaps[5].0, snaps[5].1.clone()),
        Verdict::Diverge(_)
    ));
    // Стакан перескочил время снимка без состояния с этим временем — сопоставить нельзя, расхождения нет.
    let mut c = Check::default();
    assert_eq!(
        c.on_snapshot(at, snaps[4].1.clone(), Some(at - 1)),
        Verdict::Wait
    );
    assert_eq!(c.on_local(at + 1, snaps[5].1.clone()), Verdict::Skip);
}

#[test]
fn full_book_reply_parses() {
    let t = std::fs::read_to_string(
        "tests/data/cex_samples_2026_11_04/kucoin_orderbook_level2_20_CAKE-USDT.json",
    )
    .unwrap();
    let v: Value = serde_json::from_str(&t).unwrap();
    let s = full_book::parse(&v["data"]).unwrap();
    assert_eq!(s.seq, 783409027);
    assert_eq!(s.bids.len(), 20);
    assert_eq!(s.bids[0], lv("1.892", "330.1572"));
    assert_eq!(s.asks[0], lv("1.894", "0.1946"));
}

/// Ревью решения 423 (правило ревью 420/421/424): быстрый стакан застыл на состоянии живого снимка 1793740793775, а
/// живые снимки 1793740797565 и 1793740798551 новее и не равны ему — второй подряд даёт расхождение; изменение стакана
/// между снимками счёт сбрасывает.
#[test]
fn two_newer_unequal_snapshots_without_local_change_diverge() {
    let snaps = snapshots();
    let (t4, p4) = snaps[4].clone();
    let mut c = Check::default();
    assert_eq!(c.on_local(t4, p4.clone()), Verdict::Skip);
    assert_eq!(
        c.on_snapshot(snaps[5].0, snaps[5].1.clone(), Some(t4)),
        Verdict::Wait
    );
    assert!(matches!(
        c.on_snapshot(snaps[6].0, snaps[6].1.clone(), Some(t4)),
        Verdict::Diverge(_)
    ));
    let mut c = Check::default();
    assert_eq!(c.on_local(t4, p4.clone()), Verdict::Skip);
    assert_eq!(
        c.on_snapshot(snaps[5].0, snaps[5].1.clone(), Some(t4)),
        Verdict::Wait
    );
    // Стакан изменился (то же время): снимок 1793740797565 ещё ждёт состояния своего времени, счёт сброшен.
    assert_eq!(c.on_local(t4, p4), Verdict::Wait);
    assert_eq!(
        c.on_snapshot(snaps[6].0, snaps[6].1.clone(), Some(t4)),
        Verdict::Wait
    );
}
