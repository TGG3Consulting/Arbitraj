//! Файл: tests/xt_currencies_live.rs
//! Что проверяет: решение 184 — разбор живого ответа XT GET /v4/public/currencies (05.11.2026): result — объект
//! {time, version, currencies[...]}, статусы — числа. Раньше бот ждал массив («/v4/public/currencies не массив»),
//! справочник сетей XT не обновлялся.
//! Данные: tests/data/xt_live_2026-11-05/currencies.json. Кто вызывает: cargo test.

use arb::venues::xt::networks_parse::parse_statuses;

#[test]
fn live_object_with_currencies_is_parsed() {
    let raw = std::fs::read_to_string("tests/data/xt_live_2026-11-05/currencies.json").unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let st = parse_statuses(&v["result"]).unwrap();
    assert_eq!(st.get("CAKE"), Some(&(true, true)));
    assert_eq!(st.get("BTC"), Some(&(true, true)));
}

#[test]
fn documented_array_still_parsed() {
    let v = serde_json::json!([{"currency": "cake", "depositStatus": "1", "withdrawStatus": "0"}]);
    let st = parse_statuses(&v).unwrap();
    assert_eq!(st.get("CAKE"), Some(&(true, false)));
}
