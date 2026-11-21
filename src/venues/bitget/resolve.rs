//! Файл: venues/bitget/resolve.rs
//! Что делает: решение 231 — исход операции «неизвестен» по «чем найти» из ошибки (gateway/pending.rs): ордер — по
//! orderId, если был, иначе по своему clientOid (order_state::recover: поиск; не найден — отмена по clientOid и поиск до
//! venue.order_final_wait_ms), затем обычное доведение (settle) и итог (result, монета получения — из PendingRef);
//! вывод — запись по своему clientOid в истории выводов (transfers::find, окно venue.history_window_s).
//! Кто вызывает: mod.rs (Venue::resolve_trade, Venue::resolve_withdraw) — ядро core/outcome_resolve.rs.
//! От чего зависит: order_state.rs, transfers.rs, num.rs, ctx.rs, errors.rs, gateway/pending.rs.
//! Настройки: через order_state.rs и records.rs (venue.order_poll_ms, venue.order_final_wait_ms, venue.history_window_s).
//!
//! Официальная документация:
//! - Get Order Info GET /api/v2/spot/trade/orderInfo?orderId= | clientOid= («20 times/1s (UID)») — order_state.rs.
//! - Get Withdrawal Records GET /api/v2/spot/wallet/withdrawal-records — clientOid («when passed, other parameters are
//!   ignored») — transfers.rs, records.rs.

use crate::core::types::{TradeRequest, TradeResult, Transfer, WithdrawRequest};
use crate::gateway::errors::GatewayResult;
use crate::gateway::pending::{self, PendingRef};

use super::ctx::Ctx;
use super::{errors, num, order_state, transfers};

/// Что делает: окончательный итог ордера. Принимает: окружение, исходный запрос, «чем найти».
/// Возвращает: Some(итог); None — пока не найден. Ошибки: нет символа, своего ID или монеты получения (BadResponse),
/// итог не получен (OutcomeUnknown — ядро спросит ещё), комиссия (как у trade).
pub async fn trade(
    ctx: &Ctx,
    req: TradeRequest,
    p: &PendingRef,
) -> GatewayResult<Option<TradeResult>> {
    let (Some(symbol), Some(client), Some(received)) = (
        p.symbol.as_deref(),
        p.client_id.as_deref(),
        p.received.as_deref(),
    ) else {
        return Err(errors::bad(
            "нет символа, своего ID или монеты получения",
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
    order_state::result(ctx, req, &state, order_id, received).map(Some)
}

/// Что делает: создан ли вывод. Принимает: окружение, исходный запрос, «чем найти». Возвращает: Some(перевод Pending
/// с orderId Bitget); None — пока не найден. Ошибки: нет своего ID (BadResponse).
pub async fn withdraw(
    ctx: &Ctx,
    req: WithdrawRequest,
    p: &PendingRef,
) -> GatewayResult<Option<Transfer>> {
    let Some(client) = p.client_id.as_deref() else {
        return Err(errors::bad("нет своего ID вывода", &format!("{p:?}")));
    };
    let rec = transfers::find(ctx, ("clientOid", client.to_string())).await;
    Ok(rec
        .and_then(|r| num::text_opt(&r, "orderId"))
        .map(|id| pending::created(ctx.venue(), req, id)))
}
