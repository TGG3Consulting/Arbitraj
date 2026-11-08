//! Файл: venues/binance/order_state.rs
//! Что делает: доводит ордер Binance до окончательного состояния и собирает итог ядра. Ответ FULL на ордер LIMIT IOC
//! обычно уже окончательный (FILLED / EXPIRED) — тогда без запросов. Иначе — опрос GET /api/v3/order до окончательного;
//! сбой запроса — повтор до срока; не дождался за venue.order_final_wait_ms — отмена остатка (правило Тиграна «не
//! исполнился — отменяем») и ещё опрос. Ордер с неизвестным итогом постановки ищется по своему ID
//! (origClientOrderId); не найден — отмена по нему же и снова поиск. Комиссия — fills.rs.
//! Кто вызывает: trade.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs, fills.rs.
//! Настройки: venue.order_poll_ms, venue.order_final_wait_ms; venue.my_trades_limit (fills.rs).
//!
//! Официально (rest-api «Trading endpoints», enums.md «Order status»): NEW, PENDING_NEW, PARTIALLY_FILLED, FILLED,
//! CANCELED, PENDING_CANCEL (не используется), REJECTED, EXPIRED, EXPIRED_IN_MATCH; окончательные — FILLED, CANCELED,
//! REJECTED, EXPIRED, EXPIRED_IN_MATCH. GET /api/v3/order (вес 4) — orderId или origClientOrderId; DELETE /api/v3/order —
//! «At least one of orderId or origClientOrderId»; поля executedQty, cummulativeQuoteQty, origQty.

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

/// Окончательные состояния ордера (enums.md).
const FINAL: [&str; 5] = [
    "FILLED",
    "CANCELED",
    "REJECTED",
    "EXPIRED",
    "EXPIRED_IN_MATCH",
];

/// Сроки ожидания: venue.order_poll_ms и venue.order_final_wait_ms. Ошибка NotConfigured — не задана.
pub fn times(ctx: &Ctx) -> GatewayResult<(Duration, Duration)> {
    Ok((
        Duration::from_millis(ctx.int("venue.order_poll_ms")?),
        Duration::from_millis(ctx.int("venue.order_final_wait_ms")?),
    ))
}

/// Состояние окончательное. Ошибок нет.
pub fn is_final(s: &Value) -> bool {
    num::text_opt(s, "status").is_some_and(|st| FINAL.contains(&st.as_str()))
}

/// Что делает: окончательное состояние ордера по orderId. Принимает: окружение, символ, orderId.
/// Возвращает: ответ Binance с окончательным статусом. Ошибки: OutcomeUnknown — не получено и после отмены.
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
            Ok(_) => Err(unknown(ctx, order_id, &format!("отмена принята; {why}"))),
            Err(e) => Err(unknown(
                ctx,
                order_id,
                &format!("отмена: {}; {why}", e.message),
            )),
        },
    }
}

/// Что делает: orderId по своему ID, когда итог постановки неизвестен: поиск; не найден — отмена по нему и поиск с
/// повторами до venue.order_final_wait_ms. Возвращает: orderId; None — не найден. Ошибок нет (сбои — None).
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

/// Опрос до окончательного состояния не дольше wait; сбой запроса — повтор. Ошибка — текст последней причины.
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

/// Что делает: итог ядра по окончательному состоянию. Комиссия — из fills ответа FULL, если сумма их qty равна
/// executedQty, иначе — по сделкам ордера (myTrades, fills.rs).
/// Принимает: окружение, символ, запрос ядра, состояние, orderId, отправленное количество (если нет origQty).
/// Ошибки: BadResponse — нет полей или комиссия в разных монетах; OutcomeUnknown — комиссия не подтверждена.
pub async fn result(
    ctx: &Ctx,
    symbol: &str,
    req: TradeRequest,
    s: &Value,
    order_id: String,
    sent_qty: Decimal,
) -> GatewayResult<TradeResult> {
    let orig = num::dec_opt(s, "origQty")?.unwrap_or(sent_qty);
    let filled = num::dec(s, "executedQty")?;
    let quote = num::dec(s, "cummulativeQuoteQty")?;
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
    let fees = if filled.is_zero() {
        vec![]
    } else {
        match fills::from_fills(s, filled)? {
            Some(f) => f,
            None => fills::commission(ctx, symbol, &order_id, filled).await?,
        }
    };
    Ok(TradeResult {
        request: req,
        status,
        filled_qty: filled,
        avg_price: avg,
        fees,
        venue_ref: order_id,
        chain: None,
        finished_at: Utc::now(),
        executor: None,
    })
}

/// Отмена по orderId или origClientOrderId. Ошибки: коды Binance, связь.
async fn cancel(ctx: &Ctx, symbol: &str, id: (&'static str, &str)) -> GatewayResult<Value> {
    let p = vec![("symbol", symbol.to_string()), (id.0, id.1.to_string())];
    rest::signed(ctx, Method::DELETE, "/api/v3/order", p, Op::Trade).await
}

/// Запрос состояния по orderId или origClientOrderId. Ошибки: как rest::signed.
async fn query(ctx: &Ctx, symbol: &str, id: (&'static str, &str)) -> GatewayResult<Value> {
    let q = vec![("symbol", symbol.to_string()), (id.0, id.1.to_string())];
    rest::signed(ctx, Method::GET, "/api/v3/order", q, Op::Other).await
}

/// «Ордер отправлен, итог не получен» — OutcomeUnknown (решение 64). Ошибок нет.
fn unknown(ctx: &Ctx, order_id: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        format!("Ордер {order_id} отправлен, но окончательное состояние не получено: {why}"),
    ))
}
