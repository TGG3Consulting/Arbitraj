//! Файл: tests/okx_order_result.rs
//! Что делает: решение 264 — итог ордера OKX (venues/okx/order_result.rs parse, num.rs dec_opt) на ПРИМЕРЕ ИЗ
//! ДОКУМЕНТАЦИИ «Order Book Trading → Trade → GET / Order details» https://www.okx.com/docs-v5/en/ (Response example) и
//! на случае, описанном в ней же текстом: avgPx «If none is filled, it will return ""». Живого ответа OKX в tests/data
//! нет (www.okx.com закрыт прокси; что записать — tests/README.md, раздел OKX). Плюс решение 231: пара не live —
//! запрос ордера неприменим (resolve.rs live).
//!
//! Кто вызывает: cargo test. От чего зависит: arb::venues::okx. Моков нет.

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::types::TradeStatus;
use arb::venues::okx::order_result::parse;
use arb::venues::okx::resolve::live;
use arb::venues::okx::symbols::{STATUS_OFF, STATUS_ONLINE, SymbolRules, Symbols};

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// Строка data примера документации (значения ключей — дословно из Response example раздела GET / Order details:
/// исполненный рыночный ордер на 100 USDT по BTC-USDT, tgtCcy quote_ccy).
fn docs_example() -> Value {
    json!({
        "instType": "SPOT", "instId": "BTC-USDT", "ordId": "680800019749904384", "clOrdId": "", "px": "",
        "sz": "100", "ordType": "market", "side": "buy", "tdMode": "cash", "accFillSz": "0.00192834",
        "fillPx": "51858", "fillSz": "0.00192834", "avgPx": "51858", "state": "filled",
        "fee": "-0.00000192834", "feeCcy": "BTC", "rebate": "0", "rebateCcy": "USDT", "tgtCcy": "quote_ccy",
        "cTime": "1708587373361", "uTime": "1708587373362", "cancelSource": ""
    })
}

/// Пример документации: исполнено accFillSz, средняя avgPx, комиссия −fee в feeCcy (fee отрицателен — «net fee paid
/// to platform»). Пустые строки clOrdId, px, cancelSource ошибкой не становятся.
#[test]
fn docs_example_filled() {
    let f = parse(&docs_example()).unwrap();
    assert_eq!(f.filled, d("0.00192834"));
    assert_eq!(f.avg, d("51858"));
    assert_eq!(f.fee, d("0.00000192834"));
    assert_eq!(f.fee_asset, "BTC");
}

/// Офдока: avgPx «If none is filled, it will return ""» — неисполненный IOC (state canceled, accFillSz 0) —
/// NotFilled без ошибки разбора (раньше — BadResponse «поле avgPx не число»).
#[test]
fn none_filled_avg_px_empty_is_not_filled() {
    let mut s = docs_example();
    s["ordType"] = json!("ioc");
    s["sz"] = json!("1");
    s["px"] = json!("51000");
    s["state"] = json!("canceled");
    s["accFillSz"] = json!("0");
    s["fillPx"] = json!("");
    s["fillSz"] = json!("0");
    s["avgPx"] = json!("");
    s["fee"] = json!("0");
    s["feeCcy"] = json!("");
    let f = parse(&s).unwrap();
    assert_eq!(f.status(), TradeStatus::NotFilled);
    assert_eq!(
        (f.filled, f.avg, f.fee),
        (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO)
    );
}

/// Исполнено, а комиссии нет (или avgPx пуст) — факт не подтверждён, ошибка (комиссия 0 молча не принимается).
#[test]
fn filled_without_fee_or_avg_is_error() {
    let mut s = docs_example();
    s["fee"] = json!("");
    assert!(parse(&s).is_err());
    let mut s = docs_example();
    s["avgPx"] = json!("");
    assert!(parse(&s).is_err());
}

/// «Only applicable to live instruments»: пара live — запрос ордера можно; suspend или нет в справочнике — нельзя.
#[test]
fn resolve_needs_live_instrument() {
    let rules = |state: &str, status| SymbolRules {
        symbol: "CAKE-USDT".into(),
        tick_size: d("0.001"),
        step_size: d("0.0001"),
        min_qty: None,
        max_qty: None,
        status,
        state: state.into(),
    };
    let s = Symbols::default();
    s.load(
        HashMap::from([("CAKE".into(), rules("live/normal", STATUS_ONLINE))]),
        chrono::Utc::now(),
    );
    assert!(live(&s, "CAKE-USDT").is_ok());
    assert!(live(&s, "BNB-USDT").is_err());
    s.load(
        HashMap::from([("CAKE".into(), rules("suspend/normal", STATUS_OFF))]),
        chrono::Utc::now(),
    );
    assert!(live(&s, "CAKE-USDT").is_err());
}
