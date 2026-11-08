//! Файл: venues/bybit/resolve.rs
//! Что делает: решение 231 — исход ордера «неизвестен» по «чем найти» из ошибки (gateway/pending.rs): по orderId, если
//! был, иначе по своему orderLinkId (order_state::recover: realtime, затем history; не найден — отмена по orderLinkId и
//! поиск до venue.order_final_wait_ms), затем обычное доведение (settle) и итог (trade::result).
//! Вывод не ищется: в «Get Withdrawal Records» нет поиска по своему requestId (только withdrawID, coin, время) — свой
//! вывод там находится лишь по совпадению монеты, сети, адреса и суммы (transfers.rs find, сразу после отправки);
//! однозначного способа нет — исход вывода выясняет человек (контракт по умолчанию — NotApplicable).
//! Кто вызывает: mod.rs (Venue::resolve_trade) — ядро core/outcome_resolve.rs.
//! От чего зависит: order_state.rs, trade.rs (result), ctx.rs, errors.rs, gateway/pending.rs.
//! Настройки: через order_state.rs (venue.order_poll_ms, venue.order_final_wait_ms).
//!
//! Официальная документация: «Get Open & Closed Orders» GET /v5/order/realtime — orderId | orderLinkId; «After a server
//! release or restart, filled, cancelled, and rejected orders … should only be queried through order history» —
//! GET /v5/order/history (order_state.rs).

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::GatewayResult;
use crate::gateway::pending::PendingRef;

use super::ctx::Ctx;
use super::{errors, order_state, trade};

/// Что делает: окончательный итог ордера. Принимает: окружение, исходный запрос, «чем найти».
/// Возвращает: Some(итог); None — пока не найден. Ошибки: нет символа или своего ID (BadResponse), OrderRejected —
/// найден и отклонён Bybit без исполнения (для ядра — окончательный итог), итог не получен (OutcomeUnknown — ядро
/// спросит ещё), комиссия (как у trade).
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
    let state = order_state::settle(ctx, symbol, &order_id).await;
    let out = match state {
        Ok(s) => trade::result(ctx, req, &s, order_id.clone()).await,
        Err(e) => Err(e),
    };
    ctx.private.forget(&order_id);
    out.map(Some)
}
