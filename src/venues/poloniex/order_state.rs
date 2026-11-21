//! Файл: venues/poloniex/order_state.rs
//! Что делает: доводит ордер Poloniex до окончательного состояния (FILLED / CANCELED / PARTIALLY_CANCELED / FAILED):
//! опрос по id (сбой запроса — повтор до срока); не дождался за venue.order_final_wait_ms — отмена остатка (правило
//! Тиграна) и чтение итога ещё venue.order_final_wait_ms. Ордер с неизвестным итогом постановки ищется по своему
//! clientOrderId; не найден — отмена по clientOrderId и снова поиск. Итог: filledQuantity, filledAmount, комиссия и её
//! монета — из сделок ордера (fills.rs).
//! Кто вызывает: trade.rs, order_rules.rs (times — проверка до постановки).
//! От чего зависит: rest.rs, num.rs, fills.rs, ctx.rs.
//! Настройки: venue.order_poll_ms, venue.order_final_wait_ms [poloniex].
//!
//! Официально (https://api-docs.poloniex.com/spot/api/private/order):
//! - Order Details GET /orders/{id} и /orders/cid:{clientOrderId} («50» на VIP0) — state «NEW, PARTIALLY_FILLED,
//!   FILLED, PENDING_CANCEL, PARTIALLY_CANCELED, CANCELED, FAILED»; quantity, filledQuantity, filledAmount, avgPrice
//!   («filledAmount / filledQuantity»), cancelReason. Комиссии в ордере нет.
//! - Cancel Order by Id DELETE /orders/{id} и /orders/cid:{clientOrderId} — ответ {orderId, state PENDING_CANCEL, code,
//!   message}.

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

/// Окончательные состояния ордера (перечень Order Details).
const FINAL: [&str; 4] = ["FILLED", "CANCELED", "PARTIALLY_CANCELED", "FAILED"];

/// Как искать ордер: по id биржи или по своему clientOrderId.
#[derive(Clone, Copy)]
pub enum Id<'a> {
    Order(&'a str),
    Client(&'a str),
}

impl Id<'_> {
    /// Путь запроса: /orders/{id} или /orders/cid:{clientOrderId}. Ошибок нет.
    fn path(self) -> String {
        match self {
            Id::Order(o) => format!("/orders/{o}"),
            Id::Client(c) => format!("/orders/cid:{c}"),
        }
    }
}

/// Сроки ожидания: venue.order_poll_ms и venue.order_final_wait_ms. Ошибка NotConfigured — не задана.
pub fn times(ctx: &Ctx) -> GatewayResult<(Duration, Duration)> {
    Ok((
        Duration::from_millis(ctx.int("venue.order_poll_ms")?),
        Duration::from_millis(ctx.int("venue.order_final_wait_ms")?),
    ))
}

/// Что делает: окончательное состояние ордера. Принимает: id ордера.
/// Ошибки: OutcomeUnknown — не получено даже после отмены; NotConfigured — сроки.
pub async fn settle(ctx: &Ctx, order_id: &str) -> GatewayResult<Value> {
    let (poll, wait) = times(ctx)?;
    let id = Id::Order(order_id);
    if let Ok(s) = poll_final(ctx, id, poll, wait).await {
        return Ok(s);
    }
    let cancel = cancel(ctx, id).await;
    poll_final(ctx, id, poll, wait).await.map_err(|why| {
        let c = cancel
            .err()
            .map(|e| format!("отмена: {}; ", e.message))
            .unwrap_or_default();
        unknown(ctx, order_id, &format!("{c}{why}"))
    })
}

/// Что делает: id ордера по своему clientOrderId, когда итог постановки неизвестен: поиск; не найден — отмена по
/// clientOrderId и поиск с повторами до venue.order_final_wait_ms. Возвращает: id; None — не найден. Ошибок нет.
pub async fn recover(ctx: &Ctx, client_id: &str) -> Option<String> {
    let id = Id::Client(client_id);
    let found = |s: GatewayResult<Value>| s.ok().and_then(|s| num::text_opt(&s, "id"));
    if let Some(o) = found(query(ctx, id).await) {
        return Some(o);
    }
    let (poll, wait) = times(ctx).ok()?;
    let _ = cancel(ctx, id).await;
    let started = Instant::now();
    loop {
        if let Some(o) = found(query(ctx, id).await) {
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
    id: Id<'_>,
    poll: Duration,
    wait: Duration,
) -> Result<Value, String> {
    let started = Instant::now();
    loop {
        let why = match query(ctx, id).await {
            Ok(s) => {
                let st = num::text_opt(&s, "state").unwrap_or_default();
                if FINAL.contains(&st.as_str()) {
                    return Ok(s);
                }
                format!("ордер ещё не закончен (state = {st})")
            }
            Err(e) => format!("запрос ордера: {}", e.message),
        };
        if started.elapsed() >= wait {
            return Err(why);
        }
        tokio::time::sleep(poll).await;
    }
}

/// Что делает: результат ядра по окончательному состоянию ордера и его сделкам.
/// Принимает: запрос ядра, состояние, id ордера. Комиссия — список частей по монетам сделок (решение 232).
/// Ошибки: BadResponse — нет полей; OutcomeUnknown — исполнено, а комиссия не подтверждена сделками (fills.rs).
pub async fn result(
    ctx: &Ctx,
    req: TradeRequest,
    s: &Value,
    order_id: String,
) -> GatewayResult<TradeResult> {
    let size = num::dec(s, "quantity")?;
    let filled = num::dec_opt(s, "filledQuantity")?.unwrap_or(Decimal::ZERO);
    let funds = num::dec_opt(s, "filledAmount")?.unwrap_or(Decimal::ZERO);
    let fees = if filled.is_zero() {
        Vec::new()
    } else {
        fills::fee(ctx, &order_id, filled)
            .await
            .map_err(|why| unknown(ctx, &order_id, &why))?
    };
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
        fees,
        venue_ref: order_id,
        chain: None,
        finished_at: Utc::now(),
        executor: None,
    })
}

/// Отмена по id или clientOrderId. Ошибки: коды Poloniex, связь.
async fn cancel(ctx: &Ctx, id: Id<'_>) -> GatewayResult<Value> {
    rest::signed(ctx, Method::DELETE, &id.path(), vec![], None, Op::Trade).await
}

/// Запрос ордера. Ошибки: как у rest::signed; BadResponse — ответ не объект.
pub async fn query(ctx: &Ctx, id: Id<'_>) -> GatewayResult<Value> {
    let data = rest::signed(ctx, Method::GET, &id.path(), vec![], None, Op::Other).await?;
    if data.is_object() && data.get("state").is_some() {
        return Ok(data);
    }
    Err(super::errors::bad(
        "ордера нет в ответе (нет state)",
        &data.to_string(),
    ))
}

/// «Ордер отправлен, итог не получен» — OutcomeUnknown (решение 64).
fn unknown(ctx: &Ctx, order_id: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        format!(
            "Ордер Poloniex {order_id} отправлен, но окончательное состояние не получено: {why}"
        ),
    ))
}
