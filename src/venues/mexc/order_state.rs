//! Файл: venues/mexc/order_state.rs
//! Что делает: доводит ордер MEXC до окончательного состояния: опрашивает его, пока он не станет FILLED / CANCELED /
//! PARTIALLY_CANCELED; не дождался за venue.order_final_wait_ms — отменяет остаток (правило Тиграна) и читает итог ещё
//! раз. Комиссия — по сделкам ордера (myTrades). Поиск ордера по своему ID, если итог постановки неизвестен.
//! Кто вызывает: trade.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.order_poll_ms, venue.order_final_wait_ms.
//!
//! Официальная документация:
//! - GET /api/v3/order (symbol, orderId | origClientOrderId; вес 2) https://www.mexc.com/api-docs/spot-v3/spot-account-trade/query-order
//!   — status (NEW, PARTIALLY_FILLED, FILLED, CANCELED, PARTIALLY_CANCELED), executedQty, cumulativeQuoteQty
//!   (в старом тексте — cummulativeQuoteQty: читаются оба), origQty (в живом примере — Qty: читаются оба).
//! - DELETE /api/v3/order (symbol, orderId) https://www.mexc.com/api-docs/spot-v3/spot-account-trade/cancel-order.
//! - GET /api/v3/myTrades (symbol, orderId; вес 10) https://www.mexc.com/api-docs/spot-v3/spot-account-trade/account-trade-list
//!   — commission, commissionAsset.

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
const FINAL: [&str; 3] = ["FILLED", "CANCELED", "PARTIALLY_CANCELED"];

/// Окончательное состояние ордера. Ошибка OutcomeUnknown — состояние не получено даже после отмены.
pub async fn settle(ctx: &Ctx, symbol: &str, order_id: &str) -> GatewayResult<Value> {
    let poll = Duration::from_millis(ctx.int("venue.order_poll_ms")?);
    let wait = Duration::from_millis(ctx.int("venue.order_final_wait_ms")?);
    let started = Instant::now();
    loop {
        let state = query(ctx, symbol, ("orderId", order_id)).await?;
        if is_final(&state) {
            return Ok(state);
        }
        if started.elapsed() >= wait {
            break;
        }
        tokio::time::sleep(poll).await;
    }
    let p = vec![
        ("symbol", symbol.to_string()),
        ("orderId", order_id.to_string()),
    ];
    let cancel = rest::signed(ctx, Method::DELETE, "/api/v3/order", p, Op::Trade).await;
    let state = query(ctx, symbol, ("orderId", order_id)).await?;
    if is_final(&state) {
        return Ok(state);
    }
    let why = cancel
        .err()
        .map(|e| e.message)
        .unwrap_or_else(|| "отмена принята, но статус не окончательный".into());
    Err(unknown(ctx, order_id, &why))
}

/// Ордер по своему ID (origClientOrderId) — когда ответ на постановку не получен. None — не найден или ошибка.
pub async fn find_by_client(ctx: &Ctx, symbol: &str, client_id: &str) -> Option<String> {
    query(ctx, symbol, ("origClientOrderId", client_id))
        .await
        .ok()
        .and_then(|s| num::text_opt(&s, "orderId"))
}

/// Результат ядра по окончательному состоянию; комиссия — сумма commission сделок ордера (myTrades).
/// Ошибки: BadResponse — нет полей, комиссия в нескольких монетах; сбой запроса сделок.
pub async fn result(
    ctx: &Ctx,
    symbol: &str,
    req: TradeRequest,
    s: &Value,
    order_id: String,
) -> GatewayResult<TradeResult> {
    let orig = num::dec_opt(s, "origQty")?
        .or(num::dec_opt(s, "Qty")?)
        .unwrap_or(req.qty);
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
        commission(ctx, symbol, &order_id).await?
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

/// Комиссия ордера: сумма commission его сделок и монета commissionAsset. Ошибки: связь, разные монеты комиссии.
async fn commission(ctx: &Ctx, symbol: &str, order_id: &str) -> GatewayResult<(Decimal, String)> {
    let p = vec![
        ("symbol", symbol.to_string()),
        ("orderId", order_id.to_string()),
    ];
    let data = rest::signed(ctx, Method::GET, "/api/v3/myTrades", p, Op::Other).await?;
    let trades = data
        .as_array()
        .ok_or_else(|| super::errors::bad("myTrades не массив", &data.to_string()))?;
    let (mut fee, mut asset) = (Decimal::ZERO, String::new());
    for t in trades
        .iter()
        .filter(|t| num::text_opt(t, "orderId").as_deref() == Some(order_id))
    {
        let a = num::text_opt(t, "commissionAsset").unwrap_or_default();
        if !asset.is_empty() && !a.is_empty() && a != asset {
            return Err(super::errors::bad(
                &format!("ордер {order_id}: комиссия в разных монетах ({asset}, {a})"),
                &data.to_string(),
            ));
        }
        if asset.is_empty() {
            asset = a;
        }
        fee += num::dec_opt(t, "commission")?
            .unwrap_or(Decimal::ZERO)
            .abs();
    }
    Ok((fee, asset))
}

/// Запрос состояния по orderId или origClientOrderId. Ошибка → OutcomeUnknown.
async fn query(ctx: &Ctx, symbol: &str, id: (&'static str, &str)) -> GatewayResult<Value> {
    let q = vec![("symbol", symbol.to_string()), (id.0, id.1.to_string())];
    rest::signed(ctx, Method::GET, "/api/v3/order", q, Op::Other)
        .await
        .map_err(|e| unknown(ctx, id.1, &e.message))
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
