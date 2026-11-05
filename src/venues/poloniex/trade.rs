//! Файл: venues/poloniex/trade.rs
//! Что делает: покупка или продажа на Poloniex лимитным ордером с худшей допустимой ценой и сроком IOC/FOK (что не
//! исполнилось сразу — снимает биржа); затем окончательное состояние и факт (количество, средняя цена, комиссия и её
//! монета — order_state.rs, fills.rs). Правила — order_rules.rs (та же функция у check_order, решение 134).
//! Кто вызывает: mod.rs (trade).
//! От чего зависит: order_rules.rs, order_state.rs, rest.rs, symbols.rs, num.rs, ctx.rs.
//! Настройки: через order_rules.rs и order_state.rs.
//!
//! Официально — Create Order POST /orders («50» на VIP0) https://api-docs.poloniex.com/spot/api/private/order: ответ
//! {id, clientOrderId} — это приём ордера, не исполнение (итог — Order Details). Ответ без id при успешном HTTP — итог
//! неизвестен (повтор дал бы второй ордер).

use reqwest::Method;

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{num, order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат. Ошибки: OrderRejected (правила, отказ Poloniex),
/// OutcomeUnknown — ордер мог быть принят, итог не получен (решение 64).
pub async fn trade(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = order_rules::ready(ctx, symbols, &req).await?;
    let placed = rest::signed(
        ctx,
        Method::POST,
        "/orders",
        vec![],
        Some(p.body),
        Op::Trade,
    )
    .await;
    let placed = placed.and_then(|r| {
        num::text_opt(&r, "id").ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::OutcomeUnknown,
                None,
                format!(
                    "{}: ордер принят без id в ответе — итог неизвестен",
                    p.symbol
                ),
            ))
            .with_raw(r.to_string())
        })
    });
    let order_id = match placed {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match order_state::recover(ctx, &p.client_id).await {
                Some(id) => id,
                None => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    let state = order_state::settle(ctx, &order_id).await?;
    order_state::result(ctx, req, &state, order_id, &p.received).await
}
