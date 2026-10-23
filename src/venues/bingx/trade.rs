//! Файл: venues/bingx/trade.rs
//! Что делает: покупка или продажа на BingX лимитным ордером с худшей допустимой ценой
//! и проверкой фактического исполнения (количество, средняя цена, комиссия).
//! Кто вызывает: mod.rs (trade из контракта).
//! От чего зависит: rest.rs, symbols.rs, num.rs, ctx.rs.
//! Настройки: venue.order_time_in_force (например IOC — что не исполнилось сразу, отменяется биржей);
//! ожидание итога и отмена остатка — order_state.rs.
//!
//! Официальная документация:
//! - POST /openApi/spot/v1/trade/order — symbol, side (BUY/SELL), type (LIMIT), quantity, price,
//!   timeInForce (PostOnly, GTC, IOC, FOK), newClientOrderId (буквы, цифры, «_», 1–40 символов).
//! - GET /openApi/spot/v1/trade/query — symbol, orderId; ответ: executedQty, cummulativeQuoteQty,
//!   status, fee, feeAsset, avgPrice.
//!
//! Количество округляется вниз до stepSize, цена — до tickSize в безопасную сторону
//! (покупка — вниз, продажа — вверх), сумма проверяется по minNotional (Spot trading symbols).

use reqwest::Method;
use rust_decimal::{Decimal, RoundingStrategy};

use crate::core::types::{Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{num, order_state, rest};

/// Ставит ордер и возвращает фактический результат.
/// Ждёт окончательного состояния ордера; не дождался — отменяет остаток (правило Тиграна:
/// не исполнился — отменяем, больше не пробуем) и возвращает то, что реально исполнилось.
/// Ошибки: OrderRejected (пара не торгуется, сумма вне пределов, отказ BingX),
/// Timeout — ордер отправлен, но итог не получен (нужна проверка правилом шлюза).
pub async fn trade(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let rules = symbols.get(ctx, &req.asset.ticker)?;
    let buy = req.side == Side::Buy;
    if !rules.tradable(buy) {
        return Err(reject(
            ctx,
            format!(
                "{}: пара не торгуется через API в эту сторону",
                rules.symbol
            ),
        ));
    }
    let qty = round(req.qty, rules.step_size, RoundingStrategy::ToZero);
    let price = round(
        req.worst_price,
        rules.tick_size,
        if buy {
            RoundingStrategy::ToZero
        } else {
            RoundingStrategy::AwayFromZero
        },
    );
    if qty <= Decimal::ZERO || price <= Decimal::ZERO {
        return Err(reject(
            ctx,
            format!(
                "{}: после округления по шагам количество {qty} или цена {price} не больше нуля",
                rules.symbol
            ),
        ));
    }
    if let Some(max) = rules.max_notional
        && qty * price > max
    {
        return Err(reject(
            ctx,
            format!(
                "{}: сумма {} больше максимальной {max}",
                rules.symbol,
                qty * price
            ),
        ));
    }
    if let Some(min) = rules.min_notional
        && qty * price < min
    {
        return Err(reject(
            ctx,
            format!(
                "{}: сумма {} меньше минимальной {min}",
                rules.symbol,
                qty * price
            ),
        ));
    }
    let tif = ctx.text("venue.order_time_in_force")?;
    let client_id: String = format!("{}_{}", req.deal_id, if buy { "b" } else { "s" })
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(40)
        .collect();
    let params = vec![
        ("symbol", rules.symbol.clone()),
        ("side", (if buy { "BUY" } else { "SELL" }).to_string()),
        ("type", "LIMIT".to_string()),
        ("quantity", qty.normalize().to_string()),
        ("price", price.normalize().to_string()),
        ("timeInForce", tif),
        ("newClientOrderId", client_id),
    ];
    let placed = rest::signed(
        ctx,
        Method::POST,
        "/openApi/spot/v1/trade/order",
        params,
        Op::Trade,
    )
    .await?;
    let order_id = num::text(&placed, "orderId")?;
    let state = order_state::settle(ctx, &rules.symbol, &order_id).await?;
    order_state::result(ctx, req, &state, order_id)
}

/// Округление до шага (шаг 0 — без округления).
pub fn round(v: Decimal, step: Decimal, how: RoundingStrategy) -> Decimal {
    if step.is_zero() {
        return v;
    }
    ((v / step).round_dp_with_strategy(0, how) * step).normalize()
}

/// Ошибка «ордер не принят» до отправки.
fn reject(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::OrderRejected, None, msg))
}
