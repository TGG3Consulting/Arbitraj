//! Файл: venues/coinex/order_result.rs
//! Что делает: итог ордера CoinEx в формате ядра (TradeResult) — по окончательному состоянию ордера или по его сделкам:
//! исполнено, средняя цена (сумма / количество), комиссия и её монета (order_fee.rs; со скидкой — fee_ccy сделок,
//! deals.rs), статус (не исполнен / частично / полностью — против отправленного количества).
//! Кто вызывает: order_state.rs.
//! От чего зависит: order_fee.rs, deals.rs, order_rules.rs (Prepared), num.rs, ctx.rs.
//! Настройки: через order_state.rs (venue.order_final_wait_ms — срок чтения сделок).
//!
//! Официально — Get Order Status (filled_amount, filled_value, base_fee, quote_fee, discount_fee)
//! https://docs.coinex.com/api/v2/spot/order/http/get-order-status и List User Deals (amount, price, fee, fee_ccy)
//! https://docs.coinex.com/api/v2/spot/deal/http/list-user-deals.

use std::time::Instant;

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Fee as CoreFee, TradeRequest, TradeResult, TradeStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::order_fee::{self, Fee};
use super::order_rules::Prepared;
use super::{deals, num};

/// Комиссия «нет» в получаемой монете (покупка — базовая, продажа — котируемая). Ошибок нет.
pub fn fee0(p: &Prepared) -> (Decimal, String) {
    let coin = if p.body["side"] == "buy" {
        &p.base
    } else {
        &p.quote
    };
    (Decimal::ZERO, coin.clone())
}

/// Итог по состоянию ордера; монета комиссии со скидкой — из сделок (не дольше wait_ms).
/// Ошибки: BadResponse (поля), OutcomeUnknown (комиссия исполненного ордера не подтверждена).
pub async fn from_state(
    ctx: &Ctx,
    req: TradeRequest,
    p: &Prepared,
    s: &Value,
    id: String,
    wait: std::time::Duration,
) -> GatewayResult<TradeResult> {
    let filled = num::dec_opt(s, "filled_amount")?.unwrap_or(Decimal::ZERO);
    let value = num::dec_opt(s, "filled_value")?.unwrap_or(Decimal::ZERO);
    let (_, recv) = fee0(p);
    let fee = match order_fee::from_order(s, &p.base, &p.quote, &recv) {
        Ok(Fee::Known(f, c)) => (f, c),
        Err(why) if !filled.is_zero() => return Err(unknown(ctx, &id, &why)),
        Err(_) => fee0(p),
        Ok(Fee::FromDeals) => {
            let start = num::int(s, "created_at").ok();
            let sum = deals::of_order(ctx, &p.symbol, &id, start, Instant::now() + wait)
                .await
                .map_err(|e| {
                    let why = format!("монета комиссии по сделкам: {}", e.message);
                    unknown(ctx, &id, &why)
                })?;
            one_fee(ctx, &sum, &id)?
        }
    };
    Ok(result(req, p.qty, filled, value, fee, id))
}

/// Итог по сделкам ордера. Ошибки: OutcomeUnknown — комиссия сразу в нескольких монетах.
pub fn from_deals(
    ctx: &Ctx,
    req: TradeRequest,
    p: &Prepared,
    sum: &deals::Sum,
    id: String,
) -> GatewayResult<TradeResult> {
    let fee = if sum.fees.is_empty() {
        fee0(p)
    } else {
        one_fee(ctx, sum, &id)?
    };
    Ok(result(req, p.qty, sum.filled, sum.value, fee, id))
}

/// Одна монета комиссии из сделок. Ошибка OutcomeUnknown — монет несколько (ядро принимает одну) или их нет.
fn one_fee(ctx: &Ctx, sum: &deals::Sum, id: &str) -> GatewayResult<(Decimal, String)> {
    match sum.fees.iter().collect::<Vec<_>>().as_slice() {
        [(c, f)] => Ok((**f, (*c).clone())),
        other => Err(unknown(
            ctx,
            id,
            &format!("комиссия по сделкам не в одной монете: {other:?}"),
        )),
    }
}

/// Результат ядра. Принимает: запрос ядра, отправленное количество (после округления), исполнено, сумму, комиссию,
/// order_id. Ошибок нет.
pub fn result(
    req: TradeRequest,
    ordered: Decimal,
    filled: Decimal,
    value: Decimal,
    fee: (Decimal, String),
    id: String,
) -> TradeResult {
    let status = if filled.is_zero() {
        TradeStatus::NotFilled
    } else if filled < ordered {
        TradeStatus::PartiallyFilled
    } else {
        TradeStatus::Filled
    };
    let avg = if filled.is_zero() {
        Decimal::ZERO
    } else {
        value / filled
    };
    TradeResult {
        request: req,
        status,
        filled_qty: filled,
        avg_price: avg,
        fees: CoreFee::single(fee.0, fee.1),
        venue_ref: id,
        chain: None,
        finished_at: Utc::now(),
        executor: None,
    }
}

/// «Ордер отправлен, итог не получен» — OutcomeUnknown (решение 64).
pub fn unknown(ctx: &Ctx, order_id: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        format!("Ордер CoinEx {order_id} отправлен, но окончательный итог не получен: {why}"),
    ))
}
