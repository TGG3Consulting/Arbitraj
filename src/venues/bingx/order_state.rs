//! Файл: venues/bingx/order_state.rs
//! Что делает: доводит ордер BingX до окончательного состояния: опрашивает его, пока он не станет
//! FILLED / CANCELED / FAILED; не дождался за venue.order_final_wait_ms — отменяет остаток
//! (правило Тиграна: не исполнился — отменяем, больше не пробуем) и читает итог ещё раз.
//! Затем переводит ответ BingX в результат ядра (исполнено, средняя цена, комиссия).
//! Кто вызывает: trade.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.order_poll_ms, venue.order_final_wait_ms.
//!
//! Официальная документация:
//! - GET /openApi/spot/v1/trade/query (symbol, orderId) — status: NEW, PENDING, PARTIALLY_FILLED,
//!   FILLED, CANCELED, FAILED; executedQty, cummulativeQuoteQty, fee, feeAsset.
//! - POST /openApi/spot/v1/trade/cancel (symbol, orderId).
//!
//! Приватный поток ордеров (WebSocket) даст итог быстрее опроса — следующий шаг адаптера.

use std::time::{Duration, Instant};

use chrono::Utc;
use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{TradeRequest, TradeResult, TradeStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Окончательные состояния ордера по документации.
const FINAL: [&str; 3] = ["FILLED", "CANCELED", "FAILED"];

/// Возвращает окончательное состояние ордера (ответ «Query Order details»).
/// Ошибка Timeout — состояние не удалось получить даже после отмены (нужна проверка).
pub async fn settle(ctx: &Ctx, symbol: &str, order_id: &str) -> GatewayResult<Value> {
    let poll = Duration::from_millis(ctx.int("venue.order_poll_ms")?);
    let wait = Duration::from_millis(ctx.int("venue.order_final_wait_ms")?);
    let started = Instant::now();
    loop {
        let state = query(ctx, symbol, order_id).await?;
        if is_final(&state) {
            return Ok(state);
        }
        if started.elapsed() >= wait {
            break;
        }
        tokio::time::sleep(poll).await;
    }
    // Не исполнился до конца за отведённое время — отменяем остаток.
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
    let state = query(ctx, symbol, order_id).await?;
    if is_final(&state) {
        return Ok(state);
    }
    let why = cancel
        .err()
        .map(|e| e.message)
        .unwrap_or_else(|| "отмена принята, но статус не окончательный".into());
    Err(unknown(ctx, order_id, &why))
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
        fee,
        fee_asset,
        venue_ref: order_id,
        chain: None,
        finished_at: Utc::now(),
    })
}

/// Запрос состояния. Ошибка → Timeout «ордер отправлен, состояние неизвестно».
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
    .map_err(|e| unknown(ctx, order_id, &e.message))
}

/// Состояние окончательное.
fn is_final(s: &Value) -> bool {
    num::text_opt(s, "status").is_some_and(|st| FINAL.contains(&st.as_str()))
}

/// Ошибка «состояние ордера неизвестно».
fn unknown(ctx: &Ctx, order_id: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::Timeout,
        None,
        format!("Ордер {order_id} отправлен, но окончательное состояние не получено: {why}"),
    ))
}
