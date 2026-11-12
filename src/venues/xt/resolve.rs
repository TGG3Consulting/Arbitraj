//! Файл: venues/xt/resolve.rs
//! Что делает: решение 231 — исход операции «неизвестен» по «чем найти» из ошибки (gateway/pending.rs): ордер — по
//! orderId, если был, иначе по своему clientOrderId (order_state::recover: поиск с повторами до venue.order_final_wait_ms;
//! отмены по clientOrderId в документации нет), затем обычное доведение (settle) и итог (result); вывод — запись по своему
//! clientOrderId (transfers::detail, очередь 1/с).
//! Кто вызывает: mod.rs (Venue::resolve_trade, Venue::resolve_withdraw) — ядро core/outcome_resolve.rs.
//! От чего зависит: order_state.rs, transfers.rs, num.rs, ctx.rs, errors.rs, gateway/pending.rs.
//! Настройки: через order_state.rs (venue.order_poll_ms, venue.order_final_wait_ms).
//!
//! Официальная документация: GET /v4/order?orderId=|clientOrderId= (https://doc.xt.com/docs/spot/Order/QuerySingle) — order_state.rs;
//! GET /v4/withdraw?recordId=|clientOrderId= (https://doc.xt.com/docs/spot/Deposit&Withdrawal/WithdrawDetail, «1/s/apikey») — transfers.rs.

use crate::core::types::{TradeRequest, TradeResult, Transfer, WithdrawRequest};
use crate::gateway::errors::GatewayResult;
use crate::gateway::pending::{self, PendingRef};

use super::ctx::Ctx;
use super::{errors, num, order_state, transfers};

/// Что делает: окончательный итог ордера. Принимает: окружение, исходный запрос, «чем найти».
/// Возвращает: Some(итог); None — пока не найден. Ошибки: нет своего ID (BadResponse), итог не получен (OutcomeUnknown —
/// ядро спросит ещё).
pub async fn trade(
    ctx: &Ctx,
    req: TradeRequest,
    p: &PendingRef,
) -> GatewayResult<Option<TradeResult>> {
    let Some(client) = p.client_id.as_deref() else {
        return Err(errors::bad("нет своего ID ордера", &format!("{p:?}")));
    };
    let order_id = match &p.venue_ref {
        Some(id) => id.clone(),
        None => match order_state::recover(ctx, client).await {
            Some(id) => id,
            None => return Ok(None),
        },
    };
    let state = order_state::settle(ctx, &order_id).await?;
    let qty = p.sent_qty.unwrap_or(req.qty);
    order_state::result(req, &state, order_id, qty).map(Some)
}

/// Что делает: создан ли вывод. Принимает: окружение, исходный запрос, «чем найти». Возвращает: Some(перевод Pending
/// с id XT); None — запись есть без id. Ошибки: нет своего ID (BadResponse); не найден или сбой — ошибка detail (ядро
/// спросит ещё).
pub async fn withdraw(
    ctx: &Ctx,
    req: WithdrawRequest,
    p: &PendingRef,
) -> GatewayResult<Option<Transfer>> {
    let Some(client) = p.client_id.as_deref() else {
        return Err(errors::bad("нет своего ID вывода", &format!("{p:?}")));
    };
    let rec = transfers::detail(ctx, ("clientOrderId", client.to_string())).await?;
    Ok(num::text_opt(&rec, "id").map(|id| pending::created(ctx.venue(), req, id)))
}
