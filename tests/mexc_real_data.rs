//! Файл: tests/mexc_real_data.rs
//! Что делает: разбор справочника пар MEXC на живом ответе GET /api/v3/exchangeInfo?symbols=CAKEUSDT,ATOMUSDT,UAIUSDT
//! от 2026-11-02 (tests/data/mexc_2026-11-02/exchange_info.json, получен из браузера без ключа):
//! - status приходит строкой "1", tradeSideType — числом 1: обе пары торгуются в обе стороны;
//! - шаги по решению 107: CAKE — количество 0.01 (baseAssetPrecision 2), цена 0.001 (quotePrecision 3); UAI — цена 0.0001;
//! - минимум суммы — quoteAmountPrecision ("1"), минимум количества — baseSizePrecision ("0.001"; у UAI "0");
//! - пара, не разрешённая ключу (нет в selfSymbols), через API не торгуется;
//! - orderTypes живого ответа — LIMIT, MARKET, LIMIT_MAKER (IMMEDIATE_OR_CANCEL нет — см. DECISIONS 107).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::mexc::symbols. Моков нет.

use std::collections::HashSet;

use rust_decimal::Decimal;
use serde_json::Value;

use arb::venues::mexc::symbols;

fn info() -> Value {
    let t = std::fs::read_to_string("tests/data/mexc_2026-11-02/exchange_info.json")
        .expect("файл записи");
    serde_json::from_str(&t).expect("JSON")
}

#[test]
fn exchange_info_parse() {
    let allowed: HashSet<String> = ["CAKEUSDT".to_string(), "UAIUSDT".to_string()].into();
    let map = symbols::parse_list(&info(), "USDT", &allowed).expect("разбор");
    assert_eq!(map.len(), 3);
    let cake = &map["CAKE"];
    assert_eq!(cake.symbol, "CAKEUSDT");
    assert_eq!(cake.status, symbols::STATUS_ONLINE);
    assert!(cake.tradable(true) && cake.tradable(false));
    assert_eq!(cake.step_size, Decimal::new(1, 2));
    assert_eq!(cake.tick_size, Decimal::new(1, 3));
    assert_eq!(cake.min_notional, Some(Decimal::ONE));
    assert_eq!(cake.min_qty, Some(Decimal::new(1, 3)));
    assert_eq!(map["UAI"].tick_size, Decimal::new(1, 4));
    assert_eq!(map["UAI"].min_qty, Some(Decimal::ZERO));
    // ATOM нет в selfSymbols ключа — через API не торгуется.
    assert!(!map["ATOM"].tradable(true) && !map["ATOM"].tradable(false));
}

/// Живой config/getall 2026-11-02 (журнал venue.networks): MEME20/BSC — withdrawFee "0", withdrawMin "1",
/// withdrawMax "99999999999999999999999999999999". Сеть не пропускается, предел вывода — Decimal::MAX.
#[test]
fn withdraw_max_beyond_decimal() {
    use arb::venues::mexc::wallet;
    let n: Value = serde_json::json!({
        "withdrawFee": "0", "withdrawMin": "1", "withdrawMax": "99999999999999999999999999999999"
    });
    assert_eq!(
        wallet::max_limit(&n, "withdrawMax").unwrap(),
        Some(Decimal::MAX)
    );
    let bad: Value = serde_json::json!({ "withdrawMax": "abc" });
    assert!(wallet::max_limit(&bad, "withdrawMax").is_err());
}
