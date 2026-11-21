//! Файл: venues/gate/resolve.rs
//! Что делает: решение 231 — исход операции «неизвестен» по «чем найти» из ошибки (gateway/pending.rs): ордер — по id,
//! если был, иначе по своему text (order_state::recover: среди ожидающих; не найден — отмена по text и поиск в истории
//! законченных с момента отправки, venue.order_history_limit записей), затем обычное доведение (settle) и итог (result);
//! вывод — по своему withdraw_order_id в истории выводов (transfers::find).
//! Кто вызывает: mod.rs (Venue::resolve_trade, Venue::resolve_withdraw) — ядро core/outcome_resolve.rs.
//! От чего зависит: order_state.rs, transfers.rs, ctx.rs, errors.rs, gateway/pending.rs.
//! Настройки: через order_state.rs (venue.order_poll_ms, venue.order_final_wait_ms, venue.order_history_limit).
//!
//! Официальная документация https://www.gate.com/docs/developers/apiv4/en/:
//! - GET /spot/orders/{order_id}: «Operations based on custom IDs can only be checked in pending orders. Only order ID
//!   can be used after the order is finished»; GET /spot/orders?status=finished&from= — история законченных (по text).
//! - GET /wallet/withdrawals — withdraw_order_id (transfers.rs).

use crate::core::types::{TradeRequest, TradeResult, Transfer, WithdrawRequest};
use crate::gateway::errors::GatewayResult;
use crate::gateway::pending::{self, PendingRef};

use super::ctx::Ctx;
use super::{errors, order_state, transfers};

/// Что делает: окончательный итог ордера. Принимает: окружение, исходный запрос, «чем найти».
/// Возвращает: Some(итог); None — пока не найден. Ошибки: нет символа, своего text или времени отправки (BadResponse),
/// итог не получен (OutcomeUnknown — ядро спросит ещё), комиссия (как у trade).
pub async fn trade(
    ctx: &Ctx,
    req: TradeRequest,
    p: &PendingRef,
) -> GatewayResult<Option<TradeResult>> {
    let (Some(symbol), Some(text), Some(sent)) =
        (p.symbol.as_deref(), p.client_id.as_deref(), p.sent_at_s)
    else {
        return Err(errors::bad(
            "нет символа, своего text или времени отправки",
            &format!("{p:?}"),
        ));
    };
    let order_id = match &p.venue_ref {
        Some(id) => id.clone(),
        None => match order_state::recover(ctx, symbol, text, sent).await {
            Some(id) => id,
            None => return Ok(None),
        },
    };
    let state = order_state::settle(ctx, symbol, &order_id).await?;
    let qty = p.sent_qty.unwrap_or(req.qty);
    order_state::result(ctx, req, &state, order_id, qty).map(Some)
}

/// Что делает: создан ли вывод. Принимает: окружение, исходный запрос, «чем найти». Возвращает: Some(перевод Pending
/// с ID Gate); None — пока не найден. Ошибки: нет своего ID (BadResponse).
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
