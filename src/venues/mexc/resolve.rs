//! Файл: venues/mexc/resolve.rs
//! Что делает: решение 231 — исход операции «неизвестен» по «чем найти» из ошибки (gateway/pending.rs): ордер — по ID
//! ордера, если был, иначе по своему newClientOrderId (order_state::recover: поиск; не найден — отмена по нему и поиск до
//! venue.order_final_wait_ms), затем обычное доведение (settle) и итог (result); вывод — по своему withdrawOrderId в
//! истории выводов (transfers::find). Новых запросов нет — те же функции, что у постановки.
//! Кто вызывает: mod.rs (Venue::resolve_trade, Venue::resolve_withdraw) — ядро core/outcome_resolve.rs.
//! От чего зависит: order_state.rs, transfers.rs, ctx.rs, gateway/pending.rs.
//! Настройки: через order_state.rs (venue.order_poll_ms, venue.order_final_wait_ms, venue.my_trades_limit).
//!
//! Официальная документация:
//! - GET /api/v3/order (symbol, orderId | origClientOrderId; вес 2) https://www.mexc.com/api-docs/spot-v3/spot-account-trade/query-order
//!   — «Either orderId or origClientOrderId must be sent».
//! - GET /api/v3/capital/withdraw/history — ответ содержит withdrawOrderId (transfers.rs).

use crate::core::types::{TradeRequest, TradeResult, Transfer, WithdrawRequest};
use crate::gateway::errors::GatewayResult;
use crate::gateway::pending::{self, PendingRef};

use super::ctx::Ctx;
use super::{errors, order_state, transfers};

/// Что делает: окончательный итог ордера. Принимает: окружение, исходный запрос, «чем найти».
/// Возвращает: Some(итог); None — ордер пока не найден. Ошибки: нет символа или своего ID в PendingRef (BadResponse),
/// итог не получен (OutcomeUnknown — ядро спросит ещё), комиссия (как у trade).
pub async fn trade(
    ctx: &Ctx,
    req: TradeRequest,
    p: &PendingRef,
) -> GatewayResult<Option<TradeResult>> {
    let (Some(symbol), Some(client)) = (p.symbol.as_deref(), p.client_id.as_deref()) else {
        return Err(errors::bad(
            "нет символа или своего ID ордера",
            &format!("{p:?}"),
        ));
    };
    let order_id = match &p.venue_ref {
        Some(id) => id.clone(),
        None => match order_state::recover(ctx, symbol, client).await {
            Some(id) => id,
            None => return Ok(None),
        },
    };
    let state = order_state::settle(ctx, symbol, &order_id).await?;
    let qty = p.sent_qty.unwrap_or(req.qty);
    order_state::result(ctx, symbol, req, &state, order_id, qty)
        .await
        .map(Some)
}

/// Что делает: создан ли вывод. Принимает: окружение, исходный запрос, «чем найти». Возвращает: Some(перевод Pending
/// с ID MEXC); None — пока не найден. Ошибки: нет своего ID (BadResponse).
pub async fn withdraw(
    ctx: &Ctx,
    req: WithdrawRequest,
    p: &PendingRef,
) -> GatewayResult<Option<Transfer>> {
    let Some(client) = p.client_id.as_deref() else {
        return Err(errors::bad("нет своего ID вывода", &format!("{p:?}")));
    };
    Ok(transfers::find(ctx, client)
        .await
        .map(|id| pending::created(ctx.venue(), req, id)))
}
