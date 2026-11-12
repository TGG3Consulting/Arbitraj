//! Файл: tests/ioc_263.rs
//! Что делает: решение 263 (Тигран 13.11) — сверка IOC-контрактов с офдокой, чистые функции адаптеров:
//! - Poloniex А2: снимок канала book с тем же или меньшим id не «новый» (depth::newer) — ядру не отдаётся и свежесть
//!   пары не продлевает; после переподписки (прежний id забыт) — снова новый. Кадр — пример офдоки Market Data → Book
//!   (https://api-docs.poloniex.com/spot/websocket/market-data: две записи BTC_USDT, id 123456 и 345678).
//! - Poloniex: cancelReason Order Details → строка журнала (пример офдоки Order Details с cancelReason 0 — строки нет;
//!   коды 1004, 1012, 2002 — по таблице поля той же страницы).
//! - Binance: expiryReason → строка журнала (пример ответа Query order из rest-api.md, «expiryReason»:
//!   «INSUFFICIENT_LIQUIDITY»; NONE — «The order has not expired», enums.md).
//! - MEXC А4: HTTP 5XX с кодом не из списка — не отказ, а «итог неизвестен» у ордера (Introduction, «HTTP 5XX codes
//!   indicate an issue on the MEXC server side»); код из таблицы Error Code 30016 «trading disabled».
//! - Bybit: 170035 «Submitted to the system for processing!» (Error Codes → Spot Trade) у ордера — «итог неизвестен».
//! - XT: свой clientOrderId — шаблон Submit order «^[a-zA-Z0-9_]{4,22}», уникален на каждую попытку.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::{poloniex, binance, mexc, bybit, xt}, arb::gateway::errors, arb::storage::journal.

use std::collections::{HashMap, HashSet};

use arb::gateway::errors::ErrorCode as C;
use arb::storage::journal::Level;
use arb::venues::{binance, bybit, mexc, poloniex, xt};
use serde_json::{Value, json};

/// Пример ответа канала book (офдока Poloniex, Market Data → Book): две записи одной пары.
const BOOK_DOC: &str = r#"{"channel":"book","data":[{"symbol":"BTC_USDT","createTime":1648052239156,"asks":[],"bids":[["40001.5","2.87"],["39999.4","1"]],"id":123456,"ts":1648052239192},{"symbol":"BTC_USDT","createTime":1648052239156,"asks":[],"bids":[["40001","2.87"],["39999","1"]],"id":345678,"ts":1648052239192}]}"#;

/// Пример ответа Order Details (офдока Poloniex, https://api-docs.poloniex.com/spot/api/private/order).
const ORDER_DOC: &str = r#"{"id":"21934611974062080","clientOrderId":"123","symbol":"TRX_USDC","state":"NEW","accountType":"SPOT","side":"SELL","type":"LIMIT","timeInForce":"GTC","quantity":"1.00","price":"10.00","avgPrice":"0.00","amount":"0.00","filledQuantity":"0.00","filledAmount":"0.00","createTime":1646196019020,"updateTime":1646196019020,"orderSource":"API","loan":false,"cancelReason":0}"#;

/// Пример ответа Query order (Binance rest-api.md, «Query order (USER_DATA)»), без полей SOR/peg/стоп-ордеров.
const BINANCE_QUERY_DOC: &str = r#"{"symbol":"LTCBTC","orderId":1,"orderListId":-1,"clientOrderId":"myOrder1","price":"0.1","origQty":"1.0","executedQty":"0.0","origQuoteOrderQty":"0.000000","cummulativeQuoteQty":"0.0","status":"NEW","timeInForce":"GTC","type":"LIMIT","side":"BUY","time":1499827319559,"updateTime":1499827319559,"isWorking":true,"workingTime":1499827319559,"selfTradePreventionMode":"NONE","expiryReason":"INSUFFICIENT_LIQUIDITY"}"#;

#[test]
fn poloniex_same_or_older_id_is_not_fresh() {
    let poloniex::depth::Msg::Books(list) = poloniex::depth::parse(BOOK_DOC) else {
        panic!("кадр офдоки не разобран");
    };
    let (first, second) = (&list[0], &list[1]);
    assert_eq!((first.seq, second.seq), (Some(123456), Some(345678)));
    let mut last = HashMap::new();
    assert!(poloniex::depth::newer(&mut last, first), "первый снимок");
    assert!(poloniex::depth::newer(&mut last, second), "id вырос");
    assert!(!poloniex::depth::newer(&mut last, second), "тот же id");
    assert!(
        !poloniex::depth::newer(&mut last, first),
        "id меньше прежнего"
    );
    // Переподписка пары (ws_conn.rs, Cmd::Resub): прежний id забыт — первый снимок новой подписки новый.
    last.remove("BTC_USDT");
    assert!(poloniex::depth::newer(&mut last, first));
}

#[test]
fn poloniex_cancel_reason_note() {
    let doc: Value = serde_json::from_str(ORDER_DOC).unwrap();
    assert!(
        poloniex::trade::cancel_note("TRX_USDC", "1", &doc).is_none(),
        "0 — Not applicable"
    );
    let with = |code: Value| {
        let mut v = doc.clone();
        v["cancelReason"] = code;
        poloniex::trade::cancel_note("TRX_USDC", "21934611974062080", &v)
    };
    let (level, text) = with(json!(1004)).unwrap();
    assert_eq!(level, Level::Warn);
    assert!(
        text.contains("cancelReason 1004") && text.contains("self-trade"),
        "{text}"
    );
    assert!(text.contains("EXPIRE_TAKER"), "{text}");
    let (level, text) = with(json!("1012")).unwrap();
    assert_eq!(level, Level::Info);
    assert!(text.contains("slippage"), "{text}");
    assert!(with(json!(2002)).unwrap().1.contains("offline"));
}

#[test]
fn binance_expiry_reason_note() {
    let doc: Value = serde_json::from_str(BINANCE_QUERY_DOC).unwrap();
    let m = binance::trade::expiry_note("LTCBTC", "1", &doc).unwrap();
    assert!(m.contains("expiryReason INSUFFICIENT_LIQUIDITY"), "{m}");
    assert!(m.contains("исполнено 0.0 из 1.0"), "{m}");
    let mut none = doc.clone();
    none["expiryReason"] = json!("NONE");
    assert!(binance::trade::expiry_note("LTCBTC", "1", &none).is_none());
    none.as_object_mut().unwrap().remove("expiryReason");
    assert!(binance::trade::expiry_note("LTCBTC", "1", &none).is_none());
}

#[test]
fn mexc_5xx_unlisted_code_is_unknown() {
    use mexc::errors::{Op, from_code_http, unsure};
    let raw = r#"{"code":30016,"msg":"trading disabled"}"#;
    let e = from_code_http(30016, "trading disabled", Op::Trade, raw, Some(503));
    assert_eq!(e.code, C::Timeout);
    assert!(e.message.contains("HTTP 503"), "{}", e.message);
    assert_eq!(unsure(e, Op::Trade).code, C::OutcomeUnknown);
    let e = from_code_http(30016, "trading disabled", Op::Other, raw, Some(500));
    assert_eq!(e.code, C::Unreachable);
    // Не 5XX — как было: отказ.
    let e = from_code_http(30016, "trading disabled", Op::Trade, raw, Some(400));
    assert_eq!(e.code, C::OrderRejected);
    // Код из списка при 5XX — по своему переводу (11444 у ордера — отказ).
    let off = "This feature is temporarily unavailable";
    let e = from_code_http(11444, off, Op::Trade, raw, Some(500));
    assert_eq!(e.code, C::OrderRejected);
}

#[test]
fn bybit_170035_is_unknown_for_order() {
    use bybit::errors::{Op, from_code, unsure};
    let msg = "Submitted to the system for processing!";
    let e = from_code(170035, msg, Op::Trade, "");
    assert_eq!(e.code, C::Timeout);
    assert_eq!(unsure(e, Op::Trade).code, C::OutcomeUnknown);
    assert_eq!(from_code(170035, msg, Op::Other, "").code, C::BadResponse);
}

#[test]
fn xt_client_id_fits_doc_pattern() {
    let mut seen = HashSet::new();
    for i in 0..2000 {
        let id = xt::order_rules::client_id(i % 2 == 0);
        assert!((4..=22).contains(&id.len()), "{id}");
        assert!(
            id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "{id}"
        );
        assert!(id.starts_with(if i % 2 == 0 { 'b' } else { 's' }), "{id}");
        assert!(seen.insert(id.clone()), "повтор {id}");
    }
}
