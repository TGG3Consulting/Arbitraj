//! Файл: venues/xt/trade.rs
//! Что делает: покупка или продажа на XT.com лимитным ордером IOC (или FOK) с худшей допустимой ценой — что не
//! исполнилось сразу, снимает биржа. Ответ постановки — только orderId; итог — запросом ордера (order_state.rs).
//! Правила пары — order_rules.rs: та же функция у проверки без исполнения (решение 134).
//! Кто вызывает: mod.rs (trade из контракта).
//! От чего зависит: order_rules.rs, order_state.rs, rest.rs, num.rs, ctx.rs, symbols.rs.
//! Настройки: venue.order_time_in_force, venue.order_test_first [xt]; сроки итога — order_state.rs (все проверяются до
//! отправки).
//!
//! Официально: POST /v4/order (https://doc.xt.com/docs/spot/Order/SubmitOrder, 20/s/apikey) — тело JSON, ответ result {orderId,
//! ip}; 5XX, COMMON_002/003 и обрыв — итог неизвестен → поиск по clientOrderId (order_state.rs::recover).

use reqwest::Method;

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::pending::PendingRef;

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{num, order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат. Ошибки: OrderRejected (правила, отказ XT), InsufficientBalance,
/// OutcomeUnknown — ордер мог быть принят, итог не получен (решение 64).
pub async fn trade(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = order_rules::ready(ctx, symbols, &req).await?;
    let placed = rest::signed(
        ctx,
        Method::POST,
        "/v4/order",
        vec![],
        Some(p.body),
        Op::Trade,
    )
    .await;
    // HTTP успешен, а orderId нет — итог неизвестен (повтор дал бы второй ордер).
    let placed = placed.and_then(|r| {
        num::text_opt(&r, "orderId").ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::OutcomeUnknown,
                None,
                format!(
                    "{}: ордер принят без orderId в ответе — итог неизвестен",
                    p.symbol
                ),
            ))
            .with_raw(r.to_string())
        })
    });
    // Решение 231: «чем найти» — к любому «итог неизвестен» ниже (ядро выяснит исход само, outcome_resolve.rs).
    let pending = PendingRef::order(&p.symbol, &p.client_id, p.qty);
    let order_id = match placed {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match order_state::recover(ctx, &p.client_id).await {
                Some(id) => id,
                None => return Err(e.pending(pending)),
            }
        }
        Err(e) => return Err(e),
    };
    let by_id = pending.with_ref(&order_id);
    let state = order_state::settle(ctx, &order_id)
        .await
        .map_err(|e| e.pending(by_id.clone()))?;
    order_state::result(req, &state, order_id, p.qty).map_err(|e| e.pending(by_id))
}
