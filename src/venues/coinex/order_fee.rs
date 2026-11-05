//! Файл: venues/coinex/order_fee.rs
//! Что делает: факт торговой комиссии ордера CoinEx по полям ордера base_fee / quote_fee / discount_fee: сколько и в
//! какой монете. Если монету по полям ордера назвать нельзя (комиссия в монете скидки или сразу в двух полях) — ответ
//! «смотреть сделки ордера» (deals.rs: у сделки есть fee и fee_ccy).
//! Кто вызывает: order_state.rs, tests/coinex_real_data.rs.
//! От чего зависит: num.rs.
//! Настройки: нет.
//!
//! Официально — Get Order Status https://docs.coinex.com/api/v2/spot/order/http/get-order-status: base_fee «trading fee
//! charged in the base currency», quote_fee «… in the quote currency», discount_fee «… in the deduction currency»
//! (какая это монета, поле ордера не говорит — скидка CET, урок 23; монета — fee_ccy сделок, List User Deals). Отдельного
//! поля «монета комиссии» у ордера нет. Знак значений не указан — берётся модуль (проверить на живом ответе).

use rust_decimal::Decimal;
use serde_json::Value;

use super::num;

/// Итог по полям ордера.
#[derive(Debug, Clone, PartialEq)]
pub enum Fee {
    /// Комиссия ≥ 0 и её монета.
    Known(Decimal, String),
    /// Монету не определить по полям ордера — взять из сделок (fee_ccy).
    FromDeals,
}

/// Что делает: комиссия ордера по полям.
/// Принимает: состояние ордера, базовую и котируемую монеты пары, получаемую монету (для «комиссии нет»).
/// Возвращает: Fee. Ошибки: текст причины — поле не число.
pub fn from_order(s: &Value, base: &str, quote: &str, received: &str) -> Result<Fee, String> {
    let get = |k: &str| {
        num::dec_opt(s, k)
            .map(|x| x.unwrap_or(Decimal::ZERO).abs())
            .map_err(|_| format!("{k} не число: {s}"))
    };
    let (b, q, d) = (get("base_fee")?, get("quote_fee")?, get("discount_fee")?);
    Ok(match (b.is_zero(), q.is_zero(), d.is_zero()) {
        (true, true, true) => Fee::Known(Decimal::ZERO, received.to_string()),
        (false, true, true) => Fee::Known(b, base.to_string()),
        (true, false, true) => Fee::Known(q, quote.to_string()),
        _ => Fee::FromDeals,
    })
}
