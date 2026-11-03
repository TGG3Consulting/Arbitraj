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

/// Решение 118, А-🟡7: orderTypes пары читаются из живого exchangeInfo — LIMIT, MARKET, LIMIT_MAKER
/// (IMMEDIATE_OR_CANCEL в списке нет; допустим при LIMIT — решение 133).
#[test]
fn order_types_from_exchange_info() {
    let allowed: HashSet<String> = ["CAKEUSDT".to_string()].into();
    let map = symbols::parse_list(&info(), "USDT", &allowed).expect("разбор");
    let types = map["CAKE"].order_types.clone().expect("orderTypes есть");
    assert_eq!(types, ["LIMIT", "MARKET", "LIMIT_MAKER"]);
    assert!(!types.contains(&"IMMEDIATE_OR_CANCEL".to_string()));
    // Решение 133: IOC/FOK в orderTypes MEXC не пишет, но принимает (живая сделка XPL 02.11) — допустимы при LIMIT.
    let rules = &map["CAKE"];
    assert!(rules.order_type_allowed("IMMEDIATE_OR_CANCEL"));
    assert!(rules.order_type_allowed("FILL_OR_KILL"));
    assert!(rules.order_type_allowed("LIMIT"));
    assert!(!rules.order_type_allowed("STOP_LOSS"));
}

/// Решение 118, А-🟡15: точность котируемой монеты — только точное совпадение (символ = база + тикер). У USDT —
/// наименьшая quoteAssetPrecision пар (CAKE 3, ATOM 3, UAI 4 → 3); у «T» и «DT» (хвост «USDT») — неизвестно,
/// раньше ends_with брал чужую точность. У монеты пары — baseAssetPrecision (CAKE — 2).
#[test]
fn precision_exact_quote() {
    let allowed: HashSet<String> = HashSet::new();
    let map = symbols::parse_list(&info(), "USDT", &allowed).expect("разбор");
    let s = symbols::Symbols::default();
    s.load(map, chrono::Utc::now());
    assert_eq!(s.precision_of("USDT"), Some(3));
    assert_eq!(s.precision_of("CAKE"), Some(2));
    assert_eq!(s.precision_of("T"), None);
    assert_eq!(s.precision_of("DT"), None);
}

/// Решение 118, А-🟠7: withdrawIntegerMultiple из примера официальной документации config/getall — "18" (целое: смысл
/// «шаг» не подтверждён) — шаг вывода не ставится, значение отдаётся как неясное (в журнал).
#[test]
fn withdraw_step_doc_example() {
    use arb::venues::mexc::networks_parse::withdraw_step;
    let n: Value = serde_json::json!({
        "withdrawFee": "0", "withdrawIntegerMultiple": "18", "withdrawMin": "1"
    });
    assert_eq!(withdraw_step(&n), (None, Some(Decimal::from(18))));
    let none: Value = serde_json::json!({ "withdrawFee": "0" });
    assert_eq!(withdraw_step(&none), (None, None));
}

/// Решение 118, А-🔴3, А-🟠9, А-🟡2, А-🟡3: коды ошибок из официальной таблицы Error Code.
#[test]
fn error_codes_by_doc() {
    use arb::gateway::errors::ErrorCode as C;
    use arb::venues::mexc::errors::{Op, from_code, from_http};
    assert_eq!(
        from_code(429, "Too Many Requests", Op::Trade, "").code,
        C::RateLimited
    );
    let clock = from_code(700003, "Timestamp outside of the recvWindow", Op::Trade, "");
    assert_eq!(clock.code, C::OrderRejected);
    assert!(clock.message.contains("часы"));
    assert_eq!(
        from_code(10073, "invalid Request-Time", Op::Other, "").code,
        C::BadResponse
    );
    let limit = from_code(30035, "You have triggered the order limit", Op::Trade, "");
    assert_eq!(limit.code, C::InsufficientPermissions);
    let abnormal = from_code(60005, "your account is abnormal", Op::Withdraw, "");
    assert_eq!(abnormal.code, C::InsufficientPermissions);
    let off = "This feature is temporarily unavailable";
    assert_eq!(from_code(11444, off, Op::Trade, "").code, C::OrderRejected);
    assert_eq!(
        from_code(11444, off, Op::Withdraw, "").code,
        C::WithdrawRejected
    );
    let e = from_http(429, "", Some(120));
    assert_eq!(e.code, C::RateLimited);
    assert!(e.message.contains("Retry-After 120"));
    assert_eq!(from_http(418, "", None).code, C::RateLimited);
}

/// Решение 118, Я-16 / А-🟠6: хеш депозита из примера официальной документации deposit/hisrec (без «0x») находится
/// по хешу транзакции в любом виде: с «0x», в верхнем регистре, с суффиксом «:индекс».
#[test]
fn tx_hash_same_doc_example() {
    use arb::gateway::tx_hash::same;
    let mexc = "01391d1c1397ef0a3cbb3c7f99a90846f7c8c2a8dddcdcf84f46b530dede203e1bc804";
    assert!(same(mexc, &format!("0x{}", mexc.to_uppercase())));
    assert!(same(&format!("{mexc}:0"), mexc));
    assert!(!same(mexc, &mexc[1..]));
    assert!(!same("", "0x"));
}
