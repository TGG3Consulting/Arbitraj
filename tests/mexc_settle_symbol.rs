//! Файл: tests/mexc_settle_symbol.rs
//! Что делает: расчётный круг USDC Base — сверка символа сохранённой заявки MEXC с парой справочника
//! (venues/mexc/settle_order.rs symbol_matches) на живом ответе GET /api/v3/exchangeInfo 2026-11-02
//! (tests/data/mexc_2026-11-02/exchange_info.json): символ берётся из поля symbol пары (baseAsset CAKE, quoteAsset
//! USDT → CAKEUSDT), котируемая монета расчётной пары должна совпасть с venue.quote_asset справочника.
//! Пары USDCUSDT в этой записи нет — проверяется то же правило на записанной паре.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::mexc::{symbols, settle_order}. Моков нет.

use std::collections::HashSet;

use serde_json::Value;

use arb::venues::mexc::{settle_order, symbols};

/// Записанный ответ exchangeInfo. Паника — файла нет или не JSON.
fn info() -> Value {
    let t = std::fs::read_to_string("tests/data/mexc_2026-11-02/exchange_info.json")
        .expect("файл записи");
    serde_json::from_str(&t).expect("JSON")
}

#[test]
fn settlement_symbol_from_exchange_info() {
    let map = symbols::parse_list(&info(), "USDT", &HashSet::new()).expect("разбор");
    let cake = map.get("CAKE");
    let usdt = Some("USDT");
    let m = settle_order::symbol_matches;
    assert!(m(cake, usdt, Some("CAKEUSDT"), "USDT"));
    // Другая котируемая монета расчётной пары — не та заявка.
    assert!(!m(cake, usdt, Some("CAKEUSDT"), "USDC"));
    // Символ другой пары справочника — не та заявка.
    assert!(!m(cake, usdt, Some("ATOMUSDT"), "USDT"));
    // Справочник с другой котируемой монетой, нет пары, нет символа заявки — не подтверждено.
    assert!(!m(cake, Some("USDC"), Some("CAKEUSDT"), "USDT"));
    assert!(!m(map.get("USDC"), usdt, Some("USDCUSDT"), "USDT"));
    assert!(!m(cake, usdt, None, "USDT"));
    assert!(!m(cake, None, Some("CAKEUSDT"), "USDT"));
}
