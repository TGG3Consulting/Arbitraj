//! Файл: venues/bybit/resolve.rs
//! Что делает: решение 231 — исход ордера «неизвестен» по «чем найти» из ошибки (gateway/pending.rs): по orderId, если
//! был, иначе по своему orderLinkId (order_state::recover: realtime, затем history; не найден — отмена по orderLinkId и
//! поиск до venue.order_final_wait_ms), затем обычное доведение (settle) и итог (trade::result).
//! Вывод (Тигран, 08.11, дополнение решения 231): в «Get Withdrawal Records» нет поиска по своему requestId (только
//! withdrawID, coin, время) — свой вывод ищется тем же совпадением, что адаптер делает сразу после сбоя (transfers.rs
//! matches): монета, сеть, адрес и сумма с момента отправки (PendingRef.sent_at_s). Ровно одна запись — вывод создан
//! (дальше обычный опрос статуса); ноль — пока не найден (ядро спросит ещё, за risk.outcome_max_s не нашлось —
//! человек); больше одной — однозначно не выбрать, NotApplicable (сразу человек).
//! Кто вызывает: mod.rs (Venue::resolve_trade, Venue::resolve_withdraw) — ядро core/outcome_resolve.rs.
//! От чего зависит: order_state.rs, trade.rs (result), transfers.rs (matches), ctx.rs, errors.rs, gateway/pending.rs.
//! Настройки: через order_state.rs (venue.order_poll_ms, venue.order_final_wait_ms).
//!
//! Официальная документация: «Get Open & Closed Orders» GET /v5/order/realtime — orderId | orderLinkId; «After a server
//! release or restart, filled, cancelled, and rejected orders … should only be queried through order history» —
//! GET /v5/order/history (order_state.rs). «Get Withdrawal Records» GET /v5/asset/withdraw/query-record — coin,
//! startTime, limit; rows[]: withdrawId, chain, amount, toAddress (transfers.rs).

use crate::core::types::{TradeRequest, TradeResult, Transfer, WithdrawRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::pending::{self, PendingRef};

use super::ctx::Ctx;
use super::{errors, order_state, trade, transfers};

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

/// Что делает: создан ли вывод — совпадение «монета + сеть + адрес + сумма» с момента отправки (transfers::matches).
/// Принимает: окружение, исходный запрос, «чем найти» (свой requestId — ключ, sent_at_s — начало окна).
/// Возвращает: Some(перевод Pending с withdrawId) — ровно одна запись; None — записей нет (ядро спросит ещё).
/// Ошибки: нет своего ID или времени отправки (BadResponse), больше одной записи (NotApplicable — человек), запрос к
/// Bybit (ядро спросит ещё).
pub async fn withdraw(
    ctx: &Ctx,
    req: WithdrawRequest,
    p: &PendingRef,
) -> GatewayResult<Option<Transfer>> {
    let (Some(client), Some(sent_s)) = (p.client_id.as_deref(), p.sent_at_s) else {
        return Err(errors::bad(
            "нет своего ID вывода или времени отправки",
            &format!("{p:?}"),
        ));
    };
    let ids = transfers::matches(ctx, &req, sent_s.saturating_mul(1000)).await?;
    let one = pick(client, &ids).map_err(|e| ctx.tag(e))?;
    Ok(one.map(|id| pending::created(ctx.venue(), req, id)))
}

/// Что делает: чистое решение по найденным записям (без сети, для тестов). Принимает: свой requestId (для текста),
/// withdrawId совпавших записей. Возвращает: Some(id) — ровно одна; None — ни одной. Ошибки: больше одной —
/// NotApplicable (однозначно не выбрать, исход выясняет человек).
pub fn pick(client: &str, ids: &[String]) -> GatewayResult<Option<String>> {
    match ids {
        [] => Ok(None),
        [one] => Ok(Some(one.clone())),
        many => Err(GatewayError::new(
            ErrorCode::NotApplicable,
            None,
            format!(
                "Вывод {client}: в истории Bybit {} записей с той же монетой, сетью, адресом и суммой ({}) — \
                 однозначно не выбрать",
                many.len(),
                many.join(", ")
            ),
        )),
    }
}
