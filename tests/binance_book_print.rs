//! Файл: tests/binance_book_print.rs
//! Что делает: решение 213 у Binance (Тигран 08.11, для всех бирж) — подтверждение тихой пары по содержимому на
//! ПРИМЕРАХ ИЗ ОФИЦИАЛЬНОЙ ДОКУМЕНТАЦИИ (github.com/binance/binance-spot-api-docs) и живой логике адаптера и ядра:
//! - разбор ответа GET /api/v3/depth (rest-api.md «Order book», пример lastUpdateId 1027024) — первые N уровней
//!   (venues/binance/book_print.rs), отсутствие стороны — ошибка с причиной;
//! - кадр частичного стакана (web-socket-streams.md «Partial Book Depth Streams», пример lastUpdateId 160) в обёртке
//!   комбинированного потока разбирается тем же разбором уровней, что и ответ REST: та же книга в двух форматах —
//!   Quiet (core/feed_print.rs через core/feed_confirm.rs verdict); пример ответа REST против кадра примера — книги
//!   разные — Lost. Номер (lastUpdateId) не сравнивается: у частичного стакана связи с REST в документации нет.
//!
//! Живых записей «кадр WebSocket + ответ REST в тот же момент» у Binance нет (api.binance.com из среды разработки закрыт
//! прокси; команды записи — tests/README.md, раздел Binance). Обёртка {"stream","data"} кадра — по «General WSS
//! information» (combined stream); ответ REST с уровнями кадра собран для теста и помечен. Моков нет.
//! Кто вызывает: cargo test. От чего зависит: arb::venues::binance (book_print, depth), arb::core::feed_confirm,
//! arb::gateway::feed_health.

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::feed_confirm::{Verdict, verdict};
use arb::core::types::PriceLevel;
use arb::gateway::feed_health::{BookPrint, BookPrints, BookVersion, PairHealth};
use arb::venues::binance::book_print;
use arb::venues::binance::depth::{self, Msg};

/// Пример ответа «Order book» (rest-api.md), комментарии убраны.
const REST_DOC: &str = r#"{"lastUpdateId":1027024,"bids":[["4.00000000","431.00000000"]],
"asks":[["4.00000200","12.00000000"]]}"#;
/// Пример данных «Partial Book Depth Streams» (web-socket-streams.md) в обёртке комбинированного потока.
const PARTIAL_DOC: &str = r#"{"stream":"bnbbtc@depth5@100ms","data":{"lastUpdateId":160,
"bids":[["0.0024","10"]],"asks":[["0.0026","100"]]}}"#;

fn lv(p: &str, q: &str) -> PriceLevel {
    PriceLevel {
        price: p.parse::<Decimal>().unwrap(),
        qty: q.parse::<Decimal>().unwrap(),
    }
}

/// Пара без номера кадра (Binance номер в учёт свежести не передаёт).
fn pair() -> PairHealth {
    PairHealth {
        ticker: "BNB".into(),
        last_ms: Some(1_000),
        since_ms: 0,
        last_id: None,
        last_changed_ms: None,
    }
}

/// BookVersion с отпечатками (N = 5).
fn prints(rest: BookPrint, ours: BookPrint) -> BookVersion {
    BookVersion {
        print: Some(BookPrints {
            levels: 5,
            rest,
            ours: Some(ours),
        }),
        ..Default::default()
    }
}

#[test]
fn rest_depth_docs_example() {
    let v: Value = serde_json::from_str(REST_DOC).unwrap();
    assert_eq!(
        book_print::parse(&v, 5).unwrap(),
        BookPrint {
            bids: vec![lv("4", "431")],
            asks: vec![lv("4.000002", "12")],
        }
    );
    let bad: Value = serde_json::from_str(r#"{"lastUpdateId":1,"bids":[]}"#).unwrap();
    assert!(book_print::parse(&bad, 5).is_err(), "нет asks");
}

#[test]
fn partial_frame_vs_rest_by_content() {
    let Msg::Depth { symbol, book } = depth::parse(PARTIAL_DOC) else {
        panic!("кадр примера Partial Book Depth не разобран")
    };
    assert_eq!(symbol, "bnbbtc");
    let (bids, asks) = depth::levels(&book).unwrap();
    let ours = BookPrint::top(&bids, &asks, 5);
    // Та же книга в формате ответа REST (уровни кадра примера, собрано для теста; «10» = «10.00000000»).
    let same: Value = serde_json::from_str(
        r#"{"lastUpdateId":160,"bids":[["0.00240000","10.00000000"]],"asks":[["0.00260000","100.00000000"]]}"#,
    )
    .unwrap();
    let quiet = prints(book_print::parse(&same, 5).unwrap(), ours.clone());
    assert_eq!(verdict(&pair(), &quiet, 2_000), Verdict::Quiet);
    let other: Value = serde_json::from_str(REST_DOC).unwrap();
    let lost = prints(book_print::parse(&other, 5).unwrap(), ours);
    assert!(matches!(verdict(&pair(), &lost, 2_000), Verdict::Lost(_)));
}
