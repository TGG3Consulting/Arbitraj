//! Файл: venues/kucoin/resolve.rs
//! Что делает: решение 231 — исход ордера «неизвестен» по «чем найти» из ошибки (gateway/pending.rs): по orderId, если
//! был, иначе по своему clientOid (order_state::recover: поиск; не найден — отмена по clientOid и поиск до
//! venue.order_final_wait_ms), затем обычное доведение (settle) и итог (result). Вывод здесь не ищется: своего ID у
//! вывода KuCoin нет; в расчётном круге Base вывод ищется по метке remark, сохранённой до POST (settle_withdraw.rs,
//! Venue::resolve_withdraw), — вне круга «чем найти» к ошибке вывода не прикладывается, исход выясняет человек.
//! Кто вызывает: mod.rs (Venue::resolve_trade) — ядро core/outcome_resolve.rs.
//! От чего зависит: order_state.rs, ctx.rs, errors.rs, gateway/pending.rs.
//! Настройки: через order_state.rs (venue.order_poll_ms, venue.order_final_wait_ms).
//!
//! Официальная документация: Get Order By ClientOid …/hf/orders/client-order/{clientOid}
//! https://www.kucoin.com/docs-new/api-3470182 и By OrderId (order_state.rs).

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::GatewayResult;
use crate::gateway::pending::PendingRef;

use super::ctx::Ctx;
use super::{errors, order_state};

/// Что делает: окончательный итог ордера. Принимает: окружение, исходный запрос, «чем найти».
/// Возвращает: Some(итог); None — пока не найден. Ошибки: нет символа или своего ID (BadResponse), итог не получен
/// (OutcomeUnknown — ядро спросит ещё).
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
    order_state::result(ctx, req, &state, order_id).map(Some)
}
