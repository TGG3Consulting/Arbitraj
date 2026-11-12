//! Файл: tests/order_reply_264.rs
//! Что делает: решение 264 — разбор ответов ордера по полям и примерам официальной документации (живых ответов этих
//! площадок в tests/data нет — что записать, tests/README.md):
//! - HTX (venues/htx/order_fee.rs traded): Σ filled-amount сделок matchresults («The amount which has been filled»,
//!   Get the Match Result of an Order, https://huobiapi.github.io/docs/spot/v1/en/) — с ней сверяется исполненное
//!   ордера, пустой список у исполненного ордера — не «комиссия 0».
//! - CoinEx (venues/coinex/trade.rs cancelled): ответ Cancel Order by client_id
//!   (https://docs.coinex.com/api/v2/spot/order/http/cancel-order-by-client-id) — data массив {code, data, message},
//!   во вложенном data — ордер; значения — из примера документации.
//! - Phemex (venues/phemex/trade.rs accepted): ответ Place order (HTTP PUT) — bizError 0, ordStatus «Created» (пример
//!   документации https://phemex-docs.github.io/) — принят; bizError из таблицы BizError или ordStatus «Rejected» — отказ.
//!
//! Кто вызывает: cargo test. От чего зависит: arb::venues::{htx, coinex, phemex}. Моков нет.

use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::gateway::errors::ErrorCode;
use arb::venues::coinex::trade::cancelled;
use arb::venues::htx::order_fee::traded;
use arb::venues::phemex::trade::accepted;

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// HTX: сделок нет (null или []) — 0 (у исполненного ордера это «ещё не видны», не нулевая комиссия); две сделки —
/// сумма filled-amount; сделка без filled-amount — ошибка.
#[test]
fn htx_traded_sums_filled_amount() {
    assert_eq!(traded(&Value::Null).unwrap(), Decimal::ZERO);
    assert_eq!(traded(&json!([])).unwrap(), Decimal::ZERO);
    let two = json!([
        {"filled-amount": "100", "filled-fees": "0.2", "fee-currency": "cake", "fee-deduct-currency": ""},
        {"filled-amount": "40.5", "filled-fees": "0.081", "fee-currency": "cake", "fee-deduct-currency": ""}
    ]);
    assert_eq!(traded(&two).unwrap(), d("140.5"));
    assert!(traded(&json!([{"filled-fees": "0.1"}])).is_err());
    assert!(traded(&json!({"filled-amount": "1"})).is_err());
}

/// Вложенный ордер примера документации Cancel Order by client_id (ключи и значения — из Response example).
fn coinex_order() -> Value {
    json!({
        "order_id": 13400, "market": "CETUSDT", "market_type": "MARGIN", "ccy": "CET", "side": "buy",
        "type": "limit", "amount": "10000", "price": "1", "unfilled_amount": "50241.7185224371",
        "filled_amount": "27564.87468358", "filled_value": "27564.87468358", "client_id": "client_id_1",
        "base_fee": "11.6582326221", "quote_fee": "0", "discount_fee": "0", "maker_fee_rate": "0",
        "taker_fee_rate": "0.0003", "last_filled_amount": "27564.87468358", "last_filled_price": "1",
        "created_at": 1691482451000_i64, "updated_at": 1691482451000_i64
    })
}

/// CoinEx: элемент code 0 со своим client_id — ордер найден (order_id для доведения); чужой client_id или элемент без
/// ордера — «своего ордера нет» с code и message элемента; data не массив — ошибка.
#[test]
fn coinex_cancel_reply_gives_order() {
    let data = json!([{"code": 0, "data": coinex_order(), "message": "OK"}]);
    let o = cancelled(&data, "client_id_1").unwrap();
    assert_eq!(o["order_id"], json!(13400));
    assert_eq!(o["filled_amount"], json!("27564.87468358"));

    let why = cancelled(&data, "other").unwrap_err();
    assert!(why.contains("code 0: OK"), "{why}");

    // Элемент без ордера (какие code и message бывают у элемента, таблица ответа не описывает) — не найден.
    let empty = json!([{"code": 0, "data": {}, "message": "OK"}]);
    assert!(cancelled(&empty, "client_id_1").is_err());

    assert!(cancelled(&json!({"order_id": 1}), "client_id_1").is_err());
}

/// Phemex: пример документации (bizError 0, ordStatus Created) — принят; bizError 11001 (TE_NO_ENOUGH_AVAILABLE_BALANCE)
/// — не хватает средств; bizError 11058 (TE_QTY_TOO_SMALL) или ordStatus Rejected — отказ ордера.
#[test]
fn phemex_place_reply_checked() {
    let ok = json!({"orderID": "d1", "clOrdID": "c1", "action": "New", "bizError": 0, "symbol": "sBTCUSDT",
        "side": "Buy", "ordType": "Limit", "ordStatus": "Created", "cumFeeEv": 0, "cumBaseQtyEv": 0,
        "cumQuoteQtyEv": 0, "avgPriceEp": 0});
    assert!(accepted(&ok).is_ok());

    let mut funds = ok.clone();
    funds["bizError"] = json!(11001);
    assert_eq!(
        accepted(&funds).unwrap_err().code,
        ErrorCode::InsufficientBalance
    );

    let mut qty = ok.clone();
    qty["bizError"] = json!(11058);
    assert_eq!(accepted(&qty).unwrap_err().code, ErrorCode::OrderRejected);

    let mut rej = ok;
    rej["ordStatus"] = json!("Rejected");
    assert_eq!(accepted(&rej).unwrap_err().code, ErrorCode::OrderRejected);
}
