//! Файл: venues/bingx/order_state.rs
//! Что делает: доводит ордер BingX до окончательного состояния: опрашивает его, пока он не станет
//! FILLED / CANCELED / FAILED; сбой запроса — повтор до срока (решение 263, как у Binance); не дождался за
//! venue.order_final_wait_ms — отменяет остаток (правило Тиграна: не исполнился — отменяем, больше не пробуем) и
//! опрашивает ещё раз до того же срока.
//! Затем переводит ответ BingX в результат ядра (исполнено, средняя цена, комиссия).
//! Кто вызывает: trade.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.order_poll_ms, venue.order_final_wait_ms.
//!
//! Официальная документация (https://github.com/BingX-API/api-ai-skills/blob/main/skills/spot-trade/api-reference.md,
//! «Query Single Order», «Cancel Order»):
//! - GET /openApi/spot/v1/trade/query (symbol, orderId; «10/s per UID») — status: NEW, PENDING, PARTIALLY_FILLED,
//!   FILLED, CANCELED, FAILED; executedQty, cummulativeQuoteQty, fee, feeAsset.
//! - POST /openApi/spot/v1/trade/cancel (symbol, orderId).
//!
//! Приватный поток ордеров (WebSocket) даст итог быстрее опроса — следующий шаг адаптера.

use std::time::{Duration, Instant};

use chrono::Utc;
use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Fee, TradeRequest, TradeResult, TradeStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Окончательные состояния ордера по документации.
const FINAL: [&str; 3] = ["FILLED", "CANCELED", "FAILED"];

/// Что делает: окончательное состояние ордера (ответ «Query Order details»). Принимает: окружение, символ, orderId.
/// Возвращает: ответ BingX с окончательным status. Ошибки: NotConfigured — сроки не заданы; OutcomeUnknown — состояние
/// не получено и после отмены (нужна проверка).
pub async fn settle(ctx: &Ctx, symbol: &str, order_id: &str) -> GatewayResult<Value> {
    let poll = Duration::from_millis(ctx.int("venue.order_poll_ms")?);
    let wait = Duration::from_millis(ctx.int("venue.order_final_wait_ms")?);
    if let Ok(state) = poll_final(ctx, symbol, order_id, poll, wait).await {
        return Ok(state);
    }
    // Не исполнился до конца за отведённое время (или состояние не читалось) — отменяем остаток.
    let p = vec![
        ("symbol", symbol.to_string()),
        ("orderId", order_id.to_string()),
    ];
    let cancel = rest::signed(
        ctx,
        Method::POST,
        "/openApi/spot/v1/trade/cancel",
        p,
        Op::Trade,
    )
    .await;
    match poll_final(ctx, symbol, order_id, poll, wait).await {
        Ok(state) => Ok(state),
        Err(why) => {
            let c = match cancel {
                Ok(_) => "отмена принята".to_string(),
                Err(e) => format!("отмена: {}", e.message),
            };
            Err(unknown(ctx, order_id, &format!("{c}; {why}")))
        }
    }
}

/// Решение 263: опрос до окончательного состояния не дольше wait; сбой запроса (лимит, 100500, сеть) — повтор через
/// poll, а не сразу «итог неизвестен». Ошибка — текст последней причины.
async fn poll_final(
    ctx: &Ctx,
    symbol: &str,
    order_id: &str,
    poll: Duration,
    wait: Duration,
) -> Result<Value, String> {
    let started = Instant::now();
    loop {
        let why = match query(ctx, symbol, order_id).await {
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

/// Результат ядра по окончательному состоянию.
/// Ошибка BadResponse — комиссия без валюты или нет нужных полей.
pub fn result(
    ctx: &Ctx,
    req: TradeRequest,
    s: &Value,
    order_id: String,
) -> GatewayResult<TradeResult> {
    let orig = num::dec(s, "origQty")?;
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
    let fee = num::dec_opt(s, "fee")?.unwrap_or(Decimal::ZERO).abs();
    let fee_asset = num::text_opt(s, "feeAsset").unwrap_or_default();
    if fee_asset.is_empty() && !fee.is_zero() {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!("Ордер {order_id}: комиссия без валюты"),
        )));
    }
    Ok(TradeResult {
        request: req,
        status,
        filled_qty: filled,
        avg_price: avg,
        fees: Fee::single(fee, fee_asset),
        venue_ref: order_id,
        chain: None,
        finished_at: Utc::now(),
        executor: None,
    })
}

/// Запрос состояния. Ошибки: как rest::signed (повтор и «итог неизвестен» решает poll_final / settle).
async fn query(ctx: &Ctx, symbol: &str, order_id: &str) -> GatewayResult<Value> {
    let q = vec![
        ("symbol", symbol.to_string()),
        ("orderId", order_id.to_string()),
    ];
    rest::signed(
        ctx,
        Method::GET,
        "/openApi/spot/v1/trade/query",
        q,
        Op::Other,
    )
    .await
}

/// Состояние окончательное.
fn is_final(s: &Value) -> bool {
    num::text_opt(s, "status").is_some_and(|st| FINAL.contains(&st.as_str()))
}

/// Ошибка «состояние ордера неизвестно»: ордер отправлен, итог не получен — OutcomeUnknown (решение 64: ядро не
/// повторяет, сделка ждёт проверки).
fn unknown(ctx: &Ctx, order_id: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        format!("Ордер {order_id} отправлен, но окончательное состояние не получено: {why}"),
    ))
}
