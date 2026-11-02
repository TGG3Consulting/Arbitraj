//! Файл: venues/mexc/order_state.rs
//! Что делает: доводит ордер MEXC до окончательного состояния: опрашивает его, пока он не станет FILLED / CANCELED /
//! PARTIALLY_CANCELED; сбой запроса статуса — не выход, а повтор до срока (решение 118, А-🔴2); не дождался за
//! venue.order_final_wait_ms — всегда отменяет остаток (правило Тиграна «не исполнился — отменяем») и читает итог с
//! повторами ещё venue.order_final_wait_ms. «Итог неизвестен» — только если итог не прочитан и после отмены.
//! Ордер, итог постановки которого неизвестен, ищется по своему ID; не найден — отмена по нему же и снова поиск
//! (решение 118, А-🟠4). Комиссия — fills.rs.
//! Кто вызывает: trade.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs, fills.rs.
//! Настройки: venue.order_poll_ms, venue.order_final_wait_ms; venue.my_trades_limit (fills.rs).
//!
//! Официальная документация:
//! - GET /api/v3/order (symbol, orderId | origClientOrderId; вес 2, SPOT_DEAL_READ) https://www.mexc.com/api-docs/spot-v3/spot-account-trade/query-order
//!   — status (NEW, PARTIALLY_FILLED, FILLED, CANCELED, PARTIALLY_CANCELED), executedQty, cumulativeQuoteQty
//!   (в старом тексте — cummulativeQuoteQty: читаются оба), origQty (в живом примере — Qty: читаются оба).
//! - DELETE /api/v3/order https://www.mexc.com/api-docs/spot-v3/spot-account-trade/cancel-order — «Either `orderId` or
//!   `origClientOrderId` must be sent.»; ответ — status, executedQty, cummulativeQuoteQty (запасной итог, если
//!   запросы статуса после отмены не прошли).

use std::time::{Duration, Instant};

use chrono::Utc;
use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{TradeRequest, TradeResult, TradeStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{fills, num, rest};

/// Окончательные состояния ордера по документации.
const FINAL: [&str; 3] = ["FILLED", "CANCELED", "PARTIALLY_CANCELED"];

/// Сроки ожидания: venue.order_poll_ms и venue.order_final_wait_ms. Ошибка NotConfigured — не задана.
pub fn times(ctx: &Ctx) -> GatewayResult<(Duration, Duration)> {
    Ok((
        Duration::from_millis(ctx.int("venue.order_poll_ms")?),
        Duration::from_millis(ctx.int("venue.order_final_wait_ms")?),
    ))
}

/// Что делает: окончательное состояние ордера.
/// Принимает: окружение, символ, ID ордера MEXC. Возвращает: ответ MEXC с окончательным статусом.
/// Ошибки: OutcomeUnknown — состояние не получено даже после отмены; NotConfigured — сроки не заданы.
pub async fn settle(ctx: &Ctx, symbol: &str, order_id: &str) -> GatewayResult<Value> {
    let (poll, wait) = times(ctx)?;
    let id = ("orderId", order_id);
    if let Ok(state) = poll_final(ctx, symbol, id, poll, wait).await {
        return Ok(state);
    }
    let cancel = cancel(ctx, symbol, id).await;
    match poll_final(ctx, symbol, id, poll, wait).await {
        Ok(state) => Ok(state),
        Err(why) => match cancel {
            Ok(c) if is_final(&c) => Ok(c),
            Ok(_) => Err(unknown(
                ctx,
                order_id,
                &format!("отмена принята, статус не окончательный; {why}"),
            )),
            Err(e) => Err(unknown(
                ctx,
                order_id,
                &format!("отмена: {}; {why}", e.message),
            )),
        },
    }
}

/// Что делает: ордер по своему ID (origClientOrderId), когда итог постановки неизвестен. Сначала поиск; не найден —
/// отмена по origClientOrderId (если он всё же в стакане — не будет висеть) и поиск с повторами до
/// venue.order_final_wait_ms.
/// Принимает: окружение, символ, свой ID. Возвращает: ID ордера MEXC; None — ордер не найден (итог неизвестен).
/// Ошибок нет (сбои — None).
pub async fn recover(ctx: &Ctx, symbol: &str, client_id: &str) -> Option<String> {
    let id = ("origClientOrderId", client_id);
    if let Ok(s) = query(ctx, symbol, id).await
        && let Some(o) = num::text_opt(&s, "orderId")
    {
        return Some(o);
    }
    let (poll, wait) = times(ctx).ok()?;
    let _ = cancel(ctx, symbol, id).await;
    let started = Instant::now();
    loop {
        if let Ok(s) = query(ctx, symbol, id).await
            && let Some(o) = num::text_opt(&s, "orderId")
        {
            return Some(o);
        }
        if started.elapsed() >= wait {
            return None;
        }
        tokio::time::sleep(poll).await;
    }
}

/// Что делает: опрос до окончательного состояния не дольше wait; сбой запроса — повтор.
/// Возвращает: окончательное состояние. Ошибка — текст последней причины (сбой или «не окончательный»).
async fn poll_final(
    ctx: &Ctx,
    symbol: &str,
    id: (&'static str, &str),
    poll: Duration,
    wait: Duration,
) -> Result<Value, String> {
    let started = Instant::now();
    loop {
        let why = match query(ctx, symbol, id).await {
            Ok(s) if is_final(&s) => return Ok(s),
            Ok(s) => format!("статус {}", num::text_opt(&s, "status").unwrap_or_default()),
            Err(e) => format!("запрос статуса: {}", e.message),
        };
        if started.elapsed() >= wait {
            return Err(why);
        }
        tokio::time::sleep(poll).await;
    }
}

/// Что делает: результат ядра по окончательному состоянию; комиссия — по всем сделкам ордера (fills.rs).
/// Принимает: окружение, символ, запрос ядра, состояние, ID ордера, отправленное (округлённое) количество — оно
/// берётся, если MEXC не прислал ни origQty, ни Qty (решение 118, А-🟡9).
/// Ошибки: BadResponse — нет полей, комиссия в разных монетах; OutcomeUnknown — комиссия не подтверждена.
pub async fn result(
    ctx: &Ctx,
    symbol: &str,
    req: TradeRequest,
    s: &Value,
    order_id: String,
    sent_qty: Decimal,
) -> GatewayResult<TradeResult> {
    let orig = num::dec_opt(s, "origQty")?
        .or(num::dec_opt(s, "Qty")?)
        .unwrap_or(sent_qty);
    let filled = num::dec(s, "executedQty")?;
    let quote = num::dec_opt(s, "cumulativeQuoteQty")?
        .or(num::dec_opt(s, "cummulativeQuoteQty")?)
        .ok_or_else(|| super::errors::bad("нет cumulativeQuoteQty", &s.to_string()))?;
    let status = if filled.is_zero() {
        TradeStatus::NotFilled
    } else if filled < orig {
        TradeStatus::PartiallyFilled
    } else {
        TradeStatus::Filled
    };
    let avg = if filled.is_zero() {
        Decimal::ZERO
    } else {
        quote / filled
    };
    let (fee, fee_asset) = if filled.is_zero() {
        (Decimal::ZERO, String::new())
    } else {
        fills::commission(ctx, symbol, &order_id, filled).await?
    };
    Ok(TradeResult {
        request: req,
        status,
        filled_qty: filled,
        avg_price: avg,
        fee,
        fee_asset,
        venue_ref: order_id,
        chain: None,
        finished_at: Utc::now(),
    })
}

/// Отмена ордера по orderId или origClientOrderId. Возвращает: ответ MEXC. Ошибки: коды MEXC, связь.
async fn cancel(ctx: &Ctx, symbol: &str, id: (&'static str, &str)) -> GatewayResult<Value> {
    let p = vec![("symbol", symbol.to_string()), (id.0, id.1.to_string())];
    rest::signed(ctx, Method::DELETE, "/api/v3/order", p, Op::Trade).await
}

/// Запрос состояния по orderId или origClientOrderId. Ошибки: как у rest::signed (вызывающий решает, что делать).
async fn query(ctx: &Ctx, symbol: &str, id: (&'static str, &str)) -> GatewayResult<Value> {
    let q = vec![("symbol", symbol.to_string()), (id.0, id.1.to_string())];
    rest::signed(ctx, Method::GET, "/api/v3/order", q, Op::Other).await
}

/// Состояние окончательное.
fn is_final(s: &Value) -> bool {
    num::text_opt(s, "status").is_some_and(|st| FINAL.contains(&st.as_str()))
}

/// «Ордер отправлен, итог не получен» — OutcomeUnknown (решение 64).
fn unknown(ctx: &Ctx, order_id: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        format!("Ордер {order_id} отправлен, но окончательное состояние не получено: {why}"),
    ))
}
