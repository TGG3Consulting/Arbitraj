//! Файл: tests/sell_precheck_rules.rs
//! Что делает: решение 134 — проверка ордера продажи без исполнения (контракт check_order) на записанных ответах API:
//! - MEXC, живой exchangeInfo 2026-11-02 (tests/data/mexc_2026-11-02/exchange_info.json): продажа CAKE ордером
//!   IMMEDIATE_OR_CANCEL проходит (orderTypes LIMIT, MARKET, LIMIT_MAKER — IOC при LIMIT, решение 133; живой случай
//!   WKC 03.11 — продажа отклонена по типу уже после покупки); тип вне orderTypes, пара не в selfSymbols ключа,
//!   количество 0 после округления, сумма меньше quoteAmountPrecision и больше maxQuoteAmount — отказ; округление
//!   продажи — количество вниз до шага, цена вверх;
//! - BingX, живой common/symbols 2026-10-24 (tests/data/bingx_2026-10-24/symbols.json): CAKE — успех и отказ по minQty;
//!   TAG (API закрыт), SEA (status 10) — отказ стороны; BLUAI — сумма больше maxNotional 500; шаг XEC — 1 монета;
//! - настоящий ордер и проверка зовут ОДНУ функцию правил (order_rules::ready) — по исходникам адаптеров.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::mexc (symbols, order_rules), arb::venues::bingx (symbols, order_rules). Моков нет.

use std::collections::HashSet;
use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::Side;
use arb::venues::{bingx, mexc};

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn json(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("файл записи")).expect("JSON")
}

/// Правила пар MEXC из живого exchangeInfo; ключу разрешены CAKEUSDT и UAIUSDT (ATOM — нет).
fn mexc_rules(ticker: &str) -> mexc::symbols::SymbolRules {
    let allowed: HashSet<String> = ["CAKEUSDT".to_string(), "UAIUSDT".to_string()].into();
    let info = json("tests/data/mexc_2026-11-02/exchange_info.json");
    mexc::symbols::parse_list(&info, "USDT", &allowed).expect("разбор")[ticker].clone()
}

/// Правила пары BingX из живого common/symbols 2026-10-24.
fn bingx_rules(ticker: &str) -> bingx::symbols::SymbolRules {
    let v = json("tests/data/bingx_2026-10-24/symbols.json");
    bingx::symbols::parse_list(&v["data"], "USDT").expect("разбор")[ticker].clone()
}

#[test]
fn mexc_sell_ok_and_rounding() {
    let cake = mexc_rules("CAKE");
    let ioc = "IMMEDIATE_OR_CANCEL";
    // Шаг количества 0.01, шаг цены 0.001 (решение 107): продажа — количество вниз, цена вверх.
    let (q, p) = mexc::order_rules::check_rules(&cake, Side::Sell, d("10.009"), d("2.1891"), ioc)
        .expect("продажа проходит");
    assert_eq!((q, p), (d("10"), d("2.19")));
    assert!(mexc::order_rules::check_rules(&cake, Side::Sell, d("10"), d("2.19"), "LIMIT").is_ok());
}

#[test]
fn mexc_sell_rejects() {
    let cake = mexc_rules("CAKE");
    let ioc = "IMMEDIATE_OR_CANCEL";
    let chk = |q: &str, p: &str, t: &str| {
        mexc::order_rules::check_rules(&cake, Side::Sell, d(q), d(p), t).unwrap_err()
    };
    let e = chk("10", "2.19", "STOP_LOSS");
    assert!(
        e.contains("не в orderTypes пары (LIMIT, MARKET, LIMIT_MAKER)"),
        "{e}"
    );
    let e = chk("0.009", "2.19", ioc);
    assert!(e.contains("не больше нуля"), "{e}");
    // quoteAmountPrecision "1" — минимальная сумма: 0.4 × 2.19 = 0.876.
    let e = chk("0.4", "2.19", ioc);
    assert!(e.contains("меньше минимальной 1"), "{e}");
    // maxQuoteAmount "2000000".
    let e = chk("1000000", "2.19", ioc);
    assert!(e.contains("больше максимальной 2000000"), "{e}");
    // ATOMUSDT нет в selfSymbols ключа — продажа через API невозможна.
    let atom = mexc_rules("ATOM");
    let e = mexc::order_rules::check_rules(&atom, Side::Sell, d("10"), d("3"), ioc).unwrap_err();
    assert!(e.contains("не торгуется через API"), "{e}");
}

#[test]
fn bingx_sell_ok_and_rejects() {
    let cake = bingx_rules("CAKE");
    // CAKE: stepSize 0.001, tickSize 0.001, minQty 2.672, minNotional 1.
    let (q, p) = bingx::order_rules::check_rules(&cake, Side::Sell, d("10.0009"), d("2.4991"))
        .expect("продажа проходит");
    assert_eq!((q, p), (d("10"), d("2.5")));
    let e = bingx::order_rules::check_rules(&cake, Side::Sell, d("2"), d("2.5")).unwrap_err();
    assert!(e.contains("меньше minQty 2.672"), "{e}");
    // TAG: status 1, apiStateSell false; SEA: status 10.
    for t in ["TAG", "SEA"] {
        let r = bingx_rules(t);
        let e = bingx::order_rules::check_rules(&r, Side::Sell, d("100000"), d("1")).unwrap_err();
        assert!(e.contains("не торгуется через API"), "{t}: {e}");
    }
    // BLUAI: maxNotional 500, minQty 46.1.
    let bluai = bingx_rules("BLUAI");
    let e = bingx::order_rules::check_rules(&bluai, Side::Sell, d("1000"), d("1")).unwrap_err();
    assert!(e.contains("больше maxNotional 500"), "{e}");
    // XEC: stepSize 1, minQty 189465 — дробный остаток срезается, меньше минимума — отказ.
    let xec = bingx_rules("XEC");
    let e =
        bingx::order_rules::check_rules(&xec, Side::Sell, d("189464.9"), d("0.00002")).unwrap_err();
    assert!(e.contains("количество 189464 меньше minQty 189465"), "{e}");
}

/// Настоящий ордер (trade.rs) и проверка без исполнения (check_order в mod.rs) зовут одну функцию правил
/// order_rules::ready; своих проверок пределов в trade.rs нет — правила не разойдутся.
#[test]
fn trade_and_check_share_rules() {
    let src = [
        (
            include_str!("../src/venues/mexc/trade.rs"),
            include_str!("../src/venues/mexc/mod.rs"),
        ),
        (
            include_str!("../src/venues/bingx/trade.rs"),
            include_str!("../src/venues/bingx/mod.rs"),
        ),
    ];
    for (trade, module) in src {
        assert!(trade.contains("order_rules::ready(ctx, symbols, &req)"));
        assert!(module.contains("async fn check_order"));
        assert!(module.contains("order_rules::ready(&self.ctx, &self.symbols, req)"));
        for own in [
            "min_notional",
            "max_notional",
            "min_qty",
            "tradable(",
            "order_type_allowed",
        ] {
            assert!(
                !trade.contains(own),
                "trade.rs проверяет {own} сам — правила разойдутся"
            );
        }
    }
}
