//! Файл: venues/bingx/trade.rs
//! Что делает: покупка или продажа на BingX лимитным ордером с худшей допустимой ценой
//! и проверкой фактического исполнения (количество, средняя цена, комиссия).
//! Кто вызывает: mod.rs (trade из контракта).
//! От чего зависит: order_rules.rs (правила пары — общие с проверкой ордера без исполнения, решение 134), rest.rs,
//! symbols.rs, num.rs, ctx.rs.
//! Настройки: venue.order_time_in_force (например IOC — что не исполнилось сразу, отменяется биржей);
//! ожидание итога и отмена остатка — order_state.rs.
//!
//! Официальная документация:
//! - POST /openApi/spot/v1/trade/order — symbol, side (BUY/SELL), type (LIMIT), quantity, price,
//!   timeInForce (PostOnly, GTC, IOC, FOK), newClientOrderId (буквы, цифры, «_», 1–40 символов).
//! - GET /openApi/spot/v1/trade/query — symbol, orderId; ответ: executedQty, cummulativeQuoteQty,
//!   status, fee, feeAsset, avgPrice (order_state.rs).
//!
//! Округление и пределы (stepSize, tickSize, minQty/maxQty, minNotional/maxNotional) — order_rules.rs.

use reqwest::Method;

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::pending::PendingRef;

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{num, order_rules, order_state, rest};

/// Округление до шага — order_rules.rs (здесь — для прежних вызовов, tests/bingx_real_data.rs).
pub use super::order_rules::round;

/// Ставит ордер и возвращает фактический результат. Правила пары — order_rules::ready (та же функция, что у
/// проверки ордера без исполнения перед покупкой, решение 134).
/// Ждёт окончательного состояния ордера; не дождался — отменяет остаток (правило Тиграна:
/// не исполнился — отменяем, больше не пробуем) и возвращает то, что реально исполнилось.
/// Ошибки: OrderRejected (пара не торгуется, количество или сумма вне пределов, отказ BingX),
/// OutcomeUnknown — ордер мог быть принят, итог не получен или ответ без orderId (ядро не повторяет, решение 64;
/// «чем найти» — решение 231).
pub async fn trade(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = order_rules::ready(ctx, symbols, &req)?;
    // Нет ответа на постановку — OutcomeUnknown (rest.rs, решение 64): ядро не повторяет. Решение 231: свой ID у каждой
    // постановки свой (решение 114) — ошибка несёт «чем найти» (newClientOrderId), и ядро ищет ордер само
    // (resolve.rs, Query Order details по clientOrderID); сразу здесь не ищем.
    let pending = PendingRef::order(&p.symbol, &p.client_id, p.qty);
    let placed = rest::signed(
        ctx,
        Method::POST,
        "/openApi/spot/v1/trade/order",
        p.params,
        Op::Trade,
    )
    .await
    .map_err(|e| e.pending(pending.clone()))?;
    // Решение 231 (Тигран, 08.11): HTTP успешен, а orderId нет — ордер мог быть принят, итог неизвестен (как у MEXC
    // trade.rs, решение 118): не BadResponse (по нему правила шлюза могли бы повторить — второй ордер), а OutcomeUnknown
    // с «чем найти» — ядро выяснит ордер по newClientOrderId (resolve.rs).
    let order_id = num::text_opt(&placed, "orderId").ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::OutcomeUnknown,
            None,
            format!(
                "{}: ордер принят без orderId в ответе — итог неизвестен",
                p.symbol
            ),
        ))
        .with_raw(placed.to_string())
        .pending(pending.clone())
    })?;
    let by_id = pending.with_ref(&order_id);
    let state = order_state::settle(ctx, &p.symbol, &order_id)
        .await
        .map_err(|e| e.pending(by_id.clone()))?;
    order_state::result(ctx, req, &state, order_id).map_err(|e| e.pending(by_id))
}
