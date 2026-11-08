//! Файл: tests/delist_parse.rs
//! Что делает: решение 175 — разбор делистингов адаптерами бирж на записанном ответе и на примерах официальной
//! документации (без моков):
//! - Gate: живая запись GET /spot/currency_pairs 2026-11-04 (tests/data/cex_samples_2026_11_04/gate_spot_currency_pairs.json)
//!   — 34 пары к USDT с delisting_time 1794366000 (11.11.2026 03:00 UTC), у остальных поля нет;
//! - Binance: пример ответа спецификации binance-api-swagger (spot_api.yaml, /sapi/v1/spot/delist-schedule);
//! - CoinEx: поля таблицы ответа Get Market Status и значение delisted_at из примера документации (сам пример —
//!   невалидный JSON из-за висячей запятой, поэтому здесь только его поля);
//! - BingX: живая запись common/symbols 2026-10-24 — пар в статусе 29 «pre-delisted» нет (пустой список, не «неизвестно»).
//!
//! Живые записи с ключом (Binance) — тест #[ignore], как записать — tests/README.md.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::{gate, binance, coinex, bingx}::delist, записанные данные.

use chrono::{TimeZone, Utc};
use serde_json::{Value, json};

/// Записанный файл JSON.
fn load(path: &str) -> Value {
    let text = std::fs::read_to_string(path).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn gate_recorded_pairs() {
    let data = load("tests/data/cex_samples_2026_11_04/gate_spot_currency_pairs.json");
    let got = arb::venues::gate::delist::parse(&data, "USDT").unwrap();
    assert!(!got.is_unknown());
    assert_eq!(got.items.len(), 34);
    let wag = got.find("WAG", false).unwrap();
    assert_eq!(wag.symbol, "WAG_USDT");
    assert_eq!(
        wag.trading_end,
        Some(Utc.with_ymd_and_hms(2026, 11, 11, 3, 0, 0).unwrap())
    );
    assert_eq!(wag.withdraw_end, None);
    // Пара без delisting_time в делистинг не попадает.
    assert!(got.find("ALEPH", false).is_none());
    // Другая котируемая монета — своих пар нет.
    assert!(
        arb::venues::gate::delist::parse(&data, "NOPE")
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn binance_spec_example() {
    let data = json!([{"delistTime": 1686161202000_i64, "symbol": ["ADAUSDT", "BNBUSDT"]}]);
    let got = arb::venues::binance::delist::parse(&data).unwrap();
    let at = Utc.timestamp_millis_opt(1686161202000).unwrap();
    assert_eq!(got.get("ADAUSDT"), Some(&at));
    assert_eq!(got.get("BNBUSDT"), Some(&at));
    // Нет обязательного поля — ошибка разбора, а не «делистингов нет».
    assert!(arb::venues::binance::delist::parse(&json!([{"delistTime": 1}])).is_err());
    assert!(arb::venues::binance::delist::parse(&json!({"code": -1})).is_err());
}

#[test]
fn coinex_doc_fields() {
    let data = json!([
        {"market": "CETUSDT", "base_ccy": "CET", "quote_ccy": "USDT", "status": "online",
         "delisted_at": 1819704600000_i64, "is_api_trading_available": true},
        {"market": "BTCUSDT", "base_ccy": "BTC", "quote_ccy": "USDT", "status": "online",
         "delisted_at": 0, "is_api_trading_available": true},
        {"market": "CETBTC", "base_ccy": "CET", "quote_ccy": "BTC", "status": "online",
         "delisted_at": 1819704600000_i64}
    ]);
    let got = arb::venues::coinex::delist::parse(&data, "USDT").unwrap();
    assert_eq!(got.items.len(), 1);
    let cet = got.find("CET", false).unwrap();
    assert_eq!(cet.symbol, "CETUSDT");
    assert_eq!(
        cet.trading_end,
        Some(Utc.timestamp_millis_opt(1819704600000).unwrap())
    );
}

#[test]
fn bingx_recorded_no_pre_delisted() {
    let data = load("tests/data/bingx_2026-10-24/symbols.json");
    let map = arb::venues::bingx::symbols::parse_list(&data["data"], "USDT").unwrap();
    let pre = arb::venues::bingx::symbols::with_status(
        &map,
        arb::venues::bingx::delist::STATUS_PRE_DELISTED,
    );
    let got = arb::venues::bingx::delist::build(pre);
    assert!(!got.is_unknown());
    assert!(got.items.is_empty());
}

/// Живой ответ Binance GET /sapi/v1/spot/delist-schedule (с ключом): записать на сервере бота в
/// tests/data/cex_samples_2026_11_05/binance_delist_schedule.json (команда — tests/README.md, «Binance — команды
/// записи») и снять #[ignore].
#[test]
#[ignore]
fn binance_recorded_schedule() {
    let data = load("tests/data/cex_samples_2026_11_05/binance_delist_schedule.json");
    let got = arb::venues::binance::delist::parse(&data).unwrap();
    assert!(!got.is_empty());
}
