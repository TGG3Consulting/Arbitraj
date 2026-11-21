//! Файл: tests/window_depth_diff.rs
//! Что делает: изменения стакана всей глубины в окнах (дополнение решения 125): (1) разбор кадра BingX
//! «<SYMBOL>@incrDepth» на примере сообщения из docs-v3 (Spot → «Incremental and Full Depth Information»,
//! dataUpdateDemo: уровни объектом {цена: количество}) и вид уровней массивом из api-ai-skills — оба принимаются;
//! (2) разбор снимка MEXC GET /api/v3/depth на примере ответа из документации (Market Data → Order Book);
//! (3) стакан на момент кадра (core/window_book.rs) — полный стакан из записанного кадра BingX depth20
//! (tests/data/bingx), изменения его же уровней по правилам версий: применено, устарело, разрыв — недостоверен до
//! полного, изменения до полного ждут его; (4) живые записи — #[ignore] до записи (что записать — tests/README.md).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::{bingx::tap_diff, mexc::tap_snap}, arb::core::window_book.

use arb::core::types::{PriceLevel, TapView};
use arb::core::window_book::Book;
use arb::gateway::tap::{TapData, TapStream};
use rust_decimal::Decimal;
use serde_json::Value;
use std::str::FromStr;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// docs-v3 dataUpdateDemo канала @incrDepth (объект JS → JSON: ключи-цены строками, m: !0 = true).
const DOCS_INCR: &str = r#"{"code":0,"timestamp":1769443327977,"success":true,"dataType":"BTC-USDT@incrDepth","data":{"action":"update","lastUpdateId":524997,"sourceUpdateId":15253687425,"asks":{"87554.35":"0.004666","87555.23":"0","87559.97":"3.136896","87560.71":"0.000056","87560.92":"0"},"bids":{"87555.22":"0","87554.34":"0.001949","87549.60":"0","87549.10":"0","87548.90":"0"}}}"#;

#[test]
fn bingx_incr_depth_docs_example() {
    let r = arb::venues::bingx::tap_diff::decode(DOCS_INCR).unwrap();
    assert_eq!(r.exchange_ms, Some(1769443327977));
    let TapView::Diff {
        bids,
        asks,
        full,
        from,
        to,
    } = r.view
    else {
        panic!("изменения")
    };
    assert!(!full);
    assert_eq!((from, to), (Some(524997), Some(524997)));
    assert_eq!((bids.len(), asks.len()), (5, 5));
    assert!(
        asks.iter()
            .any(|l| l.price == d("87555.23") && l.qty.is_zero()),
        "0 — уровня больше нет"
    );
    // Вид уровней по api-ai-skills — массив [[цена, количество]], время data.T.
    let arr = r#"{"code":0,"dataType":"BTC-USDT@incrDepth","data":{"action":"all","lastUpdateId":7,"T":1,"bids":[["1.5","2"]],"asks":[["1.6","3"]]}}"#;
    let r = arb::venues::bingx::tap_diff::decode(arr).unwrap();
    assert!(matches!(
        r.view,
        TapView::Diff {
            full: true,
            to: Some(7),
            ..
        }
    ));
    // Тот же разбор через адаптер (Venue::decode_tap → tap::decode).
    let v =
        arb::venues::bingx::tap::decode(TapStream::Diff, &TapData::Text(DOCS_INCR.into())).unwrap();
    assert!(matches!(v.view, TapView::Diff { .. }));
}

#[test]
fn mexc_snapshot_docs_example() {
    let docs = r#"{"lastUpdateId":1112416,"bids":[["15.00000","49999.00000"]],"asks":[["14.0000","1.0000"]]}"#;
    let r = arb::venues::mexc::tap_snap::decode_snapshot(docs).unwrap();
    let TapView::Diff {
        bids,
        asks,
        full,
        to,
        ..
    } = r.view
    else {
        panic!("снимок")
    };
    assert!(full);
    assert_eq!(to, Some(1112416));
    assert_eq!(
        bids,
        vec![PriceLevel {
            price: d("15.00000"),
            qty: d("49999.00000")
        }]
    );
    assert_eq!(asks[0].qty, d("1.0000"));
}

/// Уровни записанного кадра BingX depth20.
fn recorded_levels() -> (Vec<PriceLevel>, Vec<PriceLevel>) {
    let text = std::fs::read_to_string("tests/data/bingx/ws_spot_2026-10-23.jsonl").unwrap();
    let m = text
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["m"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .find(|m| m.contains("@depth20") && m.contains("\"data\""))
        .unwrap();
    let r = arb::venues::bingx::tap::decode(TapStream::Book, &TapData::Text(m)).unwrap();
    let TapView::Book { bids, asks } = r.view else {
        panic!("стакан")
    };
    (bids, asks)
}

fn diff(bids: Vec<PriceLevel>, asks: Vec<PriceLevel>, full: bool, from: i64, to: i64) -> TapView {
    TapView::Diff {
        bids,
        asks,
        full,
        from: Some(from),
        to: Some(to),
    }
}

#[test]
fn book_at_moment_follows_versions() {
    let (bids, asks) = recorded_levels();
    let (b0, a0) = (bids[0].clone(), asks[0].clone());
    let mut book = Book::default();
    // Изменение до полного стакана ждёт его (MEXC: снимок REST берётся после подписки).
    let early = diff(
        vec![PriceLevel {
            price: b0.price,
            qty: d("0"),
        }],
        vec![],
        false,
        10,
        11,
    );
    assert_eq!(book.apply(&early, 5).unwrap().note, "ждёт полного стакана");
    let full = book
        .apply(&diff(bids.clone(), asks.clone(), true, 10, 10), 5)
        .unwrap();
    assert!(full.ok);
    assert_eq!(
        full.version,
        Some(11),
        "ждавшее изменение применено после полного"
    );
    assert_eq!(full.depth, (bids.len() - 1, asks.len()));
    assert_ne!(
        full.bids[0].price, b0.price,
        "уровень с количеством 0 убран"
    );
    // Устаревшее — пропуск; следующее по версии — применено (новое количество).
    let old = diff(
        vec![],
        vec![PriceLevel {
            price: a0.price,
            qty: d("1"),
        }],
        false,
        9,
        11,
    );
    assert_eq!(book.apply(&old, 5).unwrap().note, "устарело — пропуск");
    let next = book
        .apply(
            &diff(
                vec![],
                vec![PriceLevel {
                    price: a0.price,
                    qty: d("7"),
                }],
                false,
                12,
                12,
            ),
            5,
        )
        .unwrap();
    assert_eq!(
        (next.note.as_str(), next.asks[0].qty),
        ("применено", d("7"))
    );
    // Разрыв версий — стакан недостоверен до следующего полного.
    let gap = book.apply(&diff(vec![], vec![], false, 20, 20), 5).unwrap();
    assert!(!gap.ok && gap.note.starts_with("разрыв версий"));
    assert!(book.apply(&diff(bids, asks, true, 30, 30), 5).unwrap().ok);
    assert!(
        book.apply(&TapView::Other("ack".into()), 5).is_none(),
        "не изменения — без стакана"
    );
}

/// Живая запись BingX @incrDepth: tests/data/bingx_incr/incr.jsonl — строки {"m": текст после GZIP}, первая — action «all».
#[test]
#[ignore = "нет живой записи канала @incrDepth — tests/README.md"]
fn recorded_bingx_incr_depth() {
    let text = std::fs::read_to_string("tests/data/bingx_incr/incr.jsonl").unwrap();
    let mut book = Book::default();
    for l in text.lines() {
        let m = serde_json::from_str::<Value>(l).unwrap()["m"]
            .as_str()
            .unwrap()
            .to_string();
        let r = arb::venues::bingx::tap_diff::decode(&m).unwrap();
        let at = book.apply(&r.view, 5).unwrap();
        assert!(at.ok, "{}", at.note);
    }
}

/// Живая запись MEXC: tests/data/mexc_ws/depth_diff.hex (кадры spot@public.aggre.depth.v3.api.pb@10ms@<SYMBOL>, hex) и
/// tests/data/mexc_ws/depth_snapshot.json (ответ /api/v3/depth, взятый после первого кадра).
#[test]
#[ignore = "нет живой записи изменений стакана MEXC — tests/README.md"]
fn recorded_mexc_depth_diff() {
    let snap = std::fs::read_to_string("tests/data/mexc_ws/depth_snapshot.json").unwrap();
    let mut book = Book::default();
    let diffs = std::fs::read_to_string("tests/data/mexc_ws/depth_diff.hex").unwrap();
    for (i, l) in diffs.lines().filter(|l| !l.trim().is_empty()).enumerate() {
        let r = arb::venues::mexc::tap_snap::decode_diff(&hex::decode(l.trim()).unwrap()).unwrap();
        book.apply(&r.view, 5);
        if i == 0 {
            let s = arb::venues::mexc::tap_snap::decode_snapshot(&snap).unwrap();
            assert!(book.apply(&s.view, 5).unwrap().ok);
        }
    }
}
