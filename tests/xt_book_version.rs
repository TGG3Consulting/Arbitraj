//! Файл: tests/xt_book_version.rs
//! Что делает: решение 213 у XT.com (Тигран 08.11, для всех бирж) — подтверждение тихой пары на ПРИМЕРАХ ИЗ
//! ОФИЦИАЛЬНОЙ ДОКУМЕНТАЦИИ (github.com/xt-com/xt4-api — исходник doc.xt.com) и живой логике адаптера и ядра:
//! - разбор ответа GET /v4/public/depth (`_market/3depth.md`) — lastUpdateId и первые N уровней (book_version.rs);
//! - depth_update — по номеру: кадр примера increDepth.md (fi 121, i 123) продолжает снимок REST по правилам «Orderbook
//!   manage» (local_book.rs), номер стакана = i кадра; ядро (core/feed_confirm.rs verdict): номер REST не больше —
//!   Quiet, больше — Lost;
//! - depth — по содержимому: снимок примера limitDepth.md и ответ REST с теми же уровнями (та же книга в двух форматах)
//!   — Quiet (core/feed_print.rs); ответ примера 3depth.md против кадра примера limitDepth.md — книги разные — Lost.
//!
//! Живых записей «кадр WebSocket + ответ REST в тот же момент» у XT нет (sapi.xt.com из среды разработки закрыт
//! прокси; tests/README.md, раздел XT — команды записи). Числа примеров — из документации, снимок с lastUpdateId 121 и
//! ответ REST с уровнями кадра собраны по её правилам и помечены в тесте. Моков нет.
//! Кто вызывает: cargo test. От чего зависит: arb::venues::xt (book_version, depth, local_book), arb::core
//! (feed_confirm, feed_print), arb::gateway::feed_health.

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::feed_confirm::{Verdict, verdict};
use arb::core::types::PriceLevel;
use arb::gateway::feed_health::{BookPrint, BookPrints, BookVersion, PairHealth};
use arb::venues::xt::book_version;
use arb::venues::xt::depth::{self, Msg};
use arb::venues::xt::local_book::{Applied, LocalBook, Update};

/// Пример ответа «Get depth data» (`_market/3depth.md`), поле result.
const REST_DOC: &str = r#"{"timestamp":1662445330524,"lastUpdateId":137333589606963580,
"bids":[["200.0000","0.996000"],["100.0000","0.001000"],["20.0000","10.000000"]],"asks":[]}"#;
/// Пример кадра «Incremental depth» (`_websocket_public/increDepth.md`), комментарии убраны.
const INCRE_DOC: &str = r#"{"topic":"depth_update","event":"depth_update@btc_usdt","data":{"s":"btc_usdt",
"fi":121,"i":123,"a":[["34000","1.2"],["34001","2.3"]],"b":[["32000","0.2"],["31000","0.5"]]}}"#;
/// Пример кадра «Limited depth» (`_websocket_public/limitDepth.md`), комментарии убраны.
const LIMIT_DOC: &str = r#"{"topic":"depth","event":"depth@btc_usdt,20","data":{"s":"btc_usdt","i":12345678,
"t":1657699200000,"a":[["34000","1.2"],["34001","2.3"]],"b":[["32000","0.2"],["31000","0.5"]]}}"#;

fn lv(p: &str, q: &str) -> PriceLevel {
    PriceLevel {
        price: p.parse::<Decimal>().unwrap(),
        qty: q.parse::<Decimal>().unwrap(),
    }
}

/// Пара с номером последнего кадра (часы бота — условные: в сравнении по номеру не участвуют).
fn pair(last_id: Option<i64>) -> PairHealth {
    PairHealth {
        ticker: "BTC".into(),
        last_ms: Some(1_000),
        since_ms: 0,
        last_id,
        last_changed_ms: None,
    }
}

#[test]
fn rest_depth_docs_example() {
    let v: Value = serde_json::from_str(REST_DOC).unwrap();
    assert_eq!(
        book_version::last_update_id(&v).unwrap(),
        137_333_589_606_963_580
    );
    let p = book_version::parse(&v, 2).unwrap();
    assert_eq!(
        p,
        BookPrint {
            bids: vec![lv("200", "0.996"), lv("100", "0.001")],
            asks: vec![],
        }
    );
    let bad: Value = serde_json::from_str(r#"{"bids":[]}"#).unwrap();
    assert!(
        book_version::last_update_id(&bad).is_err(),
        "нет lastUpdateId"
    );
}

#[test]
fn depth_update_by_number() {
    let Msg::Update { data, .. } = depth::parse(INCRE_DOC) else {
        panic!("кадр примера increDepth.md не разобран как depth_update")
    };
    let mut lb = LocalBook::default();
    let u = Update {
        first: data.fi.unwrap(),
        last: data.i,
        bids: depth::convert(&data.b, "b").unwrap(),
        asks: depth::convert(&data.a, "a").unwrap(),
    };
    assert_eq!(lb.apply(u), Applied::Waiting, "до снимка — в буфер");
    assert_eq!(lb.version(), None);
    // Снимок REST с lastUpdateId 121 (собран: «fi <= lastUpdateId+1 AND i >= lastUpdateId+1» для fi 121, i 123).
    let rest: Value = serde_json::from_str(REST_DOC).unwrap();
    let r: depth::RestDepth<'_> = serde::Deserialize::deserialize(&rest).unwrap();
    let applied = lb.snapshot(
        121,
        &depth::convert(&r.bids, "b").unwrap(),
        &depth::convert(&r.asks, "a").unwrap(),
    );
    assert_eq!(applied, Applied::Changed);
    assert_eq!(lb.version(), Some(123), "номер стакана — i кадра");
    // Ядро: REST не новее номера последнего кадра — тихая; новее — кадры потеряны.
    let at = |id: i64| BookVersion {
        id: Some(id),
        ..Default::default()
    };
    assert_eq!(
        verdict(&pair(lb.version()), &at(123), 2_000),
        Verdict::Quiet
    );
    assert!(matches!(
        verdict(&pair(lb.version()), &at(124), 2_000),
        Verdict::Lost(_)
    ));
}

#[test]
fn depth_snapshot_by_content() {
    let Msg::Snapshot { data, .. } = depth::parse(LIMIT_DOC) else {
        panic!("кадр примера limitDepth.md не разобран как depth")
    };
    let (bids, asks) = depth::snapshot_levels(&data).unwrap();
    let ours = BookPrint::top(&bids, &asks, 20);
    // Та же книга в формате ответа REST (уровни кадра примера, собрано для теста).
    let same: Value = serde_json::from_str(
        r#"{"timestamp":1657699200000,"lastUpdateId":12345678,
        "bids":[["32000.00","0.20"],["31000","0.5"]],"asks":[["34001","2.3"],["34000","1.2"]]}"#,
    )
    .unwrap();
    let quiet = BookVersion {
        print: Some(BookPrints {
            levels: 20,
            rest: book_version::parse(&same, 20).unwrap(),
            ours: Some(ours.clone()),
        }),
        ..Default::default()
    };
    // Номера у кадра depth нет (связи с REST в документации нет) — решает содержимое.
    assert_eq!(verdict(&pair(None), &quiet, 2_000), Verdict::Quiet);
    let other: Value = serde_json::from_str(REST_DOC).unwrap();
    let lost = BookVersion {
        print: Some(BookPrints {
            levels: 20,
            rest: book_version::parse(&other, 20).unwrap(),
            ours: Some(ours),
        }),
        ..Default::default()
    };
    assert!(matches!(
        verdict(&pair(None), &lost, 2_000),
        Verdict::Lost(_)
    ));
}
