//! Файл: venues/kucoin/order_state.rs
//! Что делает: доводит ордер KuCoin до окончательного состояния (active = false): опрос по orderId (сбой запроса —
//! повтор до срока); не дождался за venue.order_final_wait_ms — отмена остатка (правило Тиграна) и чтение итога ещё
//! venue.order_final_wait_ms. Ордер с неизвестным итогом постановки ищется по своему clientOid; не найден — отмена по
//! clientOid и снова поиск. Итог: dealSize, dealFunds, fee, feeCurrency самого ордера.
//! Кто вызывает: trade.rs, order_rules.rs (times — проверка до постановки).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.order_poll_ms, venue.order_final_wait_ms [kucoin].
//!
//! Официально:
//! - Get Order By OrderId GET /api/v1/hf/orders/{orderId}?symbol= (Spot, вес 2) https://www.kucoin.com/docs-new/api-3470181
//!   и By ClientOid …/hf/orders/client-order/{clientOid} https://www.kucoin.com/docs-new/api-3470182 — size, dealSize,
//!   dealFunds, fee, feeCurrency («currency used to calculate trading fee»), active («true-The status of the order is
//!   active; false-The status of the order is done»).
//! - Cancel Order By OrderId Sync DELETE /api/v1/hf/orders/sync/{orderId}?symbol= https://www.kucoin.com/docs-new/api-3470185
//!   и By ClientOid Sync …/sync/client-order/{clientOid} https://www.kucoin.com/docs-new/api-3470186.

use std::time::{Duration, Instant};

use chrono::Utc;
use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{TradeRequest, TradeResult, TradeStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::reply::Pool;
use super::{num, rest};

/// Как искать ордер: по orderId биржи или по своему clientOid.
#[derive(Clone, Copy)]
pub enum Id<'a> {
    Order(&'a str),
    Client(&'a str),
}

/// Сроки ожидания: venue.order_poll_ms и venue.order_final_wait_ms. Ошибка NotConfigured — не задана.
pub fn times(ctx: &Ctx) -> GatewayResult<(Duration, Duration)> {
    Ok((
        Duration::from_millis(ctx.int("venue.order_poll_ms")?),
        Duration::from_millis(ctx.int("venue.order_final_wait_ms")?),
    ))
}

/// Что делает: окончательное состояние ордера. Принимает: символ, orderId.
/// Ошибки: OutcomeUnknown — не получено даже после отмены; NotConfigured — сроки.
pub async fn settle(ctx: &Ctx, symbol: &str, order_id: &str) -> GatewayResult<Value> {
    let (poll, wait) = times(ctx)?;
    let id = Id::Order(order_id);
    if let Ok(s) = poll_final(ctx, symbol, id, poll, wait).await {
        return Ok(s);
    }
    let cancel = cancel(ctx, symbol, id).await;
    poll_final(ctx, symbol, id, poll, wait)
        .await
        .map_err(|why| {
            let c = cancel
                .err()
                .map(|e| format!("отмена: {}; ", e.message))
                .unwrap_or_default();
            unknown(ctx, order_id, &format!("{c}{why}"))
        })
}

/// Что делает: orderId по своему clientOid, когда итог постановки неизвестен: поиск; не найден — отмена по clientOid
/// и поиск с повторами до venue.order_final_wait_ms. Возвращает: orderId; None — не найден. Ошибок нет.
pub async fn recover(ctx: &Ctx, symbol: &str, client_oid: &str) -> Option<String> {
    let id = Id::Client(client_oid);
    if let Ok(s) = query(ctx, symbol, id).await
        && let Some(o) = num::text_opt(&s, "id")
    {
        return Some(o);
    }
    let (poll, wait) = times(ctx).ok()?;
    let _ = cancel(ctx, symbol, id).await;
    let started = Instant::now();
    loop {
        if let Ok(s) = query(ctx, symbol, id).await
            && let Some(o) = num::text_opt(&s, "id")
        {
            return Some(o);
        }
        if started.elapsed() >= wait {
            return None;
        }
        tokio::time::sleep(poll).await;
    }
}

/// Опрос до active = false не дольше wait; сбой запроса — повтор. Ошибка — текст последней причины.
async fn poll_final(
    ctx: &Ctx,
    symbol: &str,
    id: Id<'_>,
    poll: Duration,
    wait: Duration,
) -> Result<Value, String> {
    let started = Instant::now();
    loop {
        let why = match query(ctx, symbol, id).await {
            Ok(s) if s.get("active") == Some(&Value::Bool(false)) => return Ok(s),
            Ok(_) => "ордер ещё активен (active = true)".to_string(),
            Err(e) => format!("запрос ордера: {}", e.message),
        };
        if started.elapsed() >= wait {
            return Err(why);
        }
        tokio::time::sleep(poll).await;
    }
}

/// Что делает: результат ядра по окончательному состоянию ордера (dealSize, dealFunds, fee, feeCurrency).
/// Принимает: запрос ядра, состояние, orderId. Ошибки: BadResponse — нет полей; OutcomeUnknown — исполнено, а монеты
/// комиссии нет (факт не подтверждён).
pub fn result(
    ctx: &Ctx,
    req: TradeRequest,
    s: &Value,
    order_id: String,
) -> GatewayResult<TradeResult> {
    let size = num::dec(s, "size")?;
    let filled = num::dec(s, "dealSize")?;
    let funds = num::dec(s, "dealFunds")?;
    let fee = num::dec_opt(s, "fee")?.unwrap_or(Decimal::ZERO).abs();
    let fee_asset = num::text_opt(s, "feeCurrency").unwrap_or_default();
    if !filled.is_zero() && fee_asset.is_empty() {
        return Err(unknown(
            ctx,
            &order_id,
            "исполнено, а feeCurrency в ответе нет",
        ));
    }
    let status = if filled.is_zero() {
        TradeStatus::NotFilled
    } else if filled < size {
        TradeStatus::PartiallyFilled
    } else {
        TradeStatus::Filled
    };
    Ok(TradeResult {
        request: req,
        status,
        filled_qty: filled,
        avg_price: if filled.is_zero() {
            Decimal::ZERO
        } else {
            funds / filled
        },
        fee,
        fee_asset,
        venue_ref: order_id,
        chain: None,
        finished_at: Utc::now(),
        executor: None,
    })
}

/// Путь ордера по виду ID. Ошибок нет.
fn path(prefix: &str, id: Id<'_>) -> String {
    match id {
        Id::Order(o) => format!("{prefix}/{o}"),
        Id::Client(c) => format!("{prefix}/client-order/{c}"),
    }
}

/// Отмена (Sync). Ошибки: коды KuCoin, связь.
async fn cancel(ctx: &Ctx, symbol: &str, id: Id<'_>) -> GatewayResult<Value> {
    let p = path("/api/v1/hf/orders/sync", id);
    let q = vec![("symbol", symbol.to_string())];
    rest::signed(ctx, Method::DELETE, &p, q, None, Op::Trade, Pool::Spot).await
}

/// Запрос ордера. Ошибки: как у rest::signed.
pub async fn query(ctx: &Ctx, symbol: &str, id: Id<'_>) -> GatewayResult<Value> {
    let p = path("/api/v1/hf/orders", id);
    let q = vec![("symbol", symbol.to_string())];
    rest::signed(ctx, Method::GET, &p, q, None, Op::Other, Pool::Spot).await
}

/// «Ордер отправлен, итог не получен» — OutcomeUnknown (решение 64).
fn unknown(ctx: &Ctx, order_id: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        format!("Ордер KuCoin {order_id} отправлен, но окончательное состояние не получено: {why}"),
    ))
}
