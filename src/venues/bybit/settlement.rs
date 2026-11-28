//! Файл: venues/bybit/settlement.rs
//! Что делает: расчётный круг Base для Bybit — ордер конверсии (USDCUSDT): сверка сохранённой заявки с расчётной парой
//! (matches_symbol) и окончательное исполнение сохранённого ордера ТОЛЬКО чтением (inspect: GET по своему orderLinkId,
//! без отмены и без новой отправки). Открытый или не найденный ордер — None (ядро спросит ещё).
//! Кто вызывает: mod.rs (Venue::matches_settlement_order, Venue::inspect_settlement_order) — ядро
//! core/settlement_effect.rs, core/settlement_target.rs, core/cash_order*.rs.
//! От чего зависит: order_state.rs (query, is_final), trade.rs (result), num.rs, ctx.rs, gateway/pending.rs.
//! Настройки: через trade.rs/fills.rs (venue.order_poll_ms, venue.order_final_wait_ms, venue.my_trades_limit) —
//! комиссия ордера из исполнений; новых нет.
//!
//! Официальная документация:
//! - «Get Open & Closed Orders» GET /v5/order/realtime https://bybit-exchange.github.io/docs/v5/order/open-order —
//!   category, orderLinkId («openOnly param will be ignored when query by orderId or orderLinkId»); «After a server
//!   release or restart, filled, cancelled, and rejected orders of Unified account should only be queried through order
//!   history»; поля orderId, orderLinkId, symbol, side («Buy», «Sell»), qty, orderStatus, cumExecQty, cumExecValue
//!   («Cumulative executed order value»), cumFeeDetail.
//! - «Get Order History» GET /v5/order/history https://bybit-exchange.github.io/docs/v5/order/order-list — orderLinkId;
//!   «Cancelled», «Rejected» без исполнения видны только «last 24 hours» — позже такой ордер не найти (None, ядро ждёт).
//! - Символ пары: «Get Instruments Info» https://bybit-exchange.github.io/docs/v5/market/instrument — symbol,
//!   baseCoin, quoteCoin. Что symbol спота = baseCoin + quoteCoin — факт записанного ответа (все 520 пар,
//!   tests/data/cex_samples_2026_11_04/bybit_instruments_spot.json, в т.ч. USDCUSDT), не правило документации.

use crate::core::types::{Side, TradeRequest, TradeResult};
use crate::gateway::errors::GatewayResult;
use crate::gateway::pending::PendingRef;

use super::ctx::Ctx;
use super::{num, order_state, trade};

/// Что делает: относится ли сохранённая заявка к спот-паре asset/quote. Принимает: символ заявки (PendingRef.symbol),
/// тикер базовой и котируемой монеты Bybit. Возвращает: true — символ ровно baseCoin + quoteCoin. Ошибок нет.
pub fn matches_symbol(symbol: Option<&str>, asset: &str, quote: &str) -> bool {
    !asset.is_empty() && !quote.is_empty() && symbol == Some(format!("{asset}{quote}").as_str())
}

/// Что делает: сторона ордера Bybit для стороны ядра (enum side: «Buy», «Sell»). Ошибок нет.
fn side_text(side: Side) -> &'static str {
    match side {
        Side::Buy => "Buy",
        Side::Sell => "Sell",
    }
}

/// Что делает: окончательное исполнение сохранённого ордера только чтением. Принимает: исходный запрос ядра, «чем
/// найти» (символ, свой orderLinkId, отправленное количество, orderId — если был). Возвращает: Some(итог) — ордер
/// окончательный и совпал по символу, orderLinkId, стороне, количеству и orderId; None — не найден, ещё открыт или
/// реквизиты не совпали (ядро спросит ещё). Ошибки: запрос к Bybit (связь, коды), OrderRejected — Rejected без
/// исполнения (окончательный итог, как у trade::result), OutcomeUnknown — комиссия не подтверждена, BadResponse.
pub async fn inspect(
    ctx: &Ctx,
    req: TradeRequest,
    p: &PendingRef,
) -> GatewayResult<Option<TradeResult>> {
    let (Some(symbol), Some(client)) = (p.symbol.as_deref(), p.client_id.as_deref()) else {
        return Ok(None);
    };
    let Some(s) = order_state::query(ctx, ("orderLinkId", client)).await? else {
        return Ok(None);
    };
    if !same_order(&s, &req, p, symbol, client)? {
        return Ok(None);
    }
    let Some(id) = num::text_opt(&s, "orderId") else {
        return Ok(None);
    };
    let out = trade::result(ctx, req, &s, id.clone()).await;
    ctx.private.forget(&id);
    out.map(Some)
}

/// Что делает: чистая проверка записи ордера против сохранённой заявки (без сети, для тестов). Принимает: запись
/// ордера Bybit, запрос ядра, «чем найти», символ и свой orderLinkId. Возвращает: true — окончательный статус и
/// совпали symbol, orderLinkId, side, qty (если сохранено) и orderId (если сохранён). Ошибки: BadResponse — qty не число.
pub fn same_order(
    s: &serde_json::Value,
    req: &TradeRequest,
    p: &PendingRef,
    symbol: &str,
    client: &str,
) -> GatewayResult<bool> {
    if !order_state::is_final(s)
        || num::text_opt(s, "symbol").as_deref() != Some(symbol)
        || num::text_opt(s, "orderLinkId").as_deref() != Some(client)
        || num::text_opt(s, "side").as_deref() != Some(side_text(req.side))
    {
        return Ok(false);
    }
    if let Some(sent) = p.sent_qty
        && num::dec(s, "qty")? != sent
    {
        return Ok(false);
    }
    let id = num::text_opt(s, "orderId");
    Ok(id.is_some()
        && p.venue_ref
            .as_ref()
            .is_none_or(|known| Some(known) == id.as_ref()))
}
