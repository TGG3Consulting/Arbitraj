//! Файл: venues/binance/trade.rs
//! Что делает: покупка или продажа на Binance лимитным ордером IOC (или FOK) с худшей допустимой ценой — что не
//! исполнилось сразу, снимает биржа (статус EXPIRED). Итог берётся из ответа FULL (status, executedQty,
//! cummulativeQuoteQty, fills с комиссией); если ответ не окончательный — опрос и отмена (order_state.rs).
//! Правила пары и пробный ордер — order_rules.rs: та же функция у проверки без исполнения (решение 134).
//! Кто вызывает: mod.rs (trade из контракта).
//! От чего зависит: order_rules.rs, order_state.rs, rest.rs, num.rs, ctx.rs, symbols.rs.
//! Настройки: venue.order_time_in_force, venue.order_test_first [binance]; сроки итога — order_state.rs;
//! venue.my_trades_limit — fills.rs (все проверяются до отправки).
//!
//! Официально: POST /api/v3/order (вес 1, «Unfilled order count: 1») — ответ FULL: orderId, status, origQty,
//! executedQty, cummulativeQuoteQty, fills[] (price, qty, commission, commissionAsset); 5XX и −1007 TIMEOUT — «execution
//! status unknown» → поиск по origClientOrderId (order_state.rs::recover).

use reqwest::Method;

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{num, order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат. Ошибки: OrderRejected (правила, пробный ордер, отказ Binance),
/// InsufficientBalance, OutcomeUnknown — ордер мог быть принят, итог не получен (решение 64).
pub async fn trade(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = order_rules::ready(ctx, symbols, &req).await?;
    let placed = rest::signed(ctx, Method::POST, "/api/v3/order", p.params, Op::Trade).await;
    // HTTP успешен, а orderId нет — итог неизвестен (повтор дал бы второй ордер).
    let placed = placed.and_then(|r| match num::text_opt(&r, "orderId") {
        Some(id) => Ok((id, r)),
        None => Err(ctx
            .tag(GatewayError::new(
                ErrorCode::OutcomeUnknown,
                None,
                format!(
                    "{}: ордер принят без orderId в ответе — итог неизвестен",
                    p.symbol
                ),
            ))
            .with_raw(r.to_string())),
    });
    let (order_id, reply) = match placed {
        Ok((id, r)) => (id, Some(r)),
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match order_state::recover(ctx, &p.symbol, &p.client_id).await {
                Some(id) => (id, None),
                None => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    let state = match reply {
        Some(r) if order_state::is_final(&r) => r,
        _ => order_state::settle(ctx, &p.symbol, &order_id).await?,
    };
    order_state::result(ctx, &p.symbol, req, &state, order_id, p.qty).await
}
