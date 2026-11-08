//! Файл: venues/htx/order_state.rs
//! Что делает: доводит ордер HTX до окончательного состояния (filled / partial-canceled / canceled): опрос по order-id
//! (сбой запроса — повтор до срока); не дождался за venue.order_final_wait_ms — отмена остатка (правило Тиграна) и
//! чтение итога ещё venue.order_final_wait_ms. Ордер с неизвестным итогом постановки ищется по своему client-order-id;
//! не найден — отмена по client-order-id и снова поиск. Итог: количество, сумма, комиссия и её монета (order_fee.rs).
//! Кто вызывает: trade.rs, order_rules.rs (times — проверка до постановки).
//! От чего зависит: rest.rs, num.rs, order_fee.rs, ctx.rs.
//! Настройки: venue.order_poll_ms, venue.order_final_wait_ms [htx].
//!
//! Официально (https://huobiapi.github.io/docs/spot/v1/en/, раздел Trading):
//! - Get the Order Detail of an Order GET /v1/order/orders/{order-id} и Get the Order Detail of an Order (based on client
//!   order ID) GET /v1/order/orders/getClientOrder?clientOrderId= («50 times/2s»): state created, submitted,
//!   partial-filled, filled, partial-canceled, canceling, canceled; amount, filled-amount, filled-cash-amount,
//!   filled-fees. В официальном SDK (pkg/model/order/getorderresponse.go) те же поля — field-amount, field-cash-amount,
//!   field-fees: расхождение (урок 2) — принимаются оба написания, проверить на живом ответе. «No longer queryable after
//!   being cancelled for 2 hours»; по client-order-id — ордера за 8 часов: итог читается сразу после постановки.
//! - Submit Cancel for an Order POST /v1/order/orders/{order-id}/submitcancel (тело пустое, как в SDK) и
//!   POST /v1/order/orders/submitCancelClientOrder {"client-order-id"} — только приём отмены, итог — запросом ордера.

use std::time::{Duration, Instant};

use chrono::Utc;
use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::{Fee, TradeRequest, TradeResult, TradeStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, order_fee, rest};

/// Окончательные состояния ордера (перечень документации).
const FINAL: [&str; 3] = ["filled", "partial-canceled", "canceled"];

/// Как искать ордер: по order-id биржи или по своему client-order-id.
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

/// Что делает: окончательное состояние ордера по order-id. Ошибки: OutcomeUnknown — не получено и после отмены.
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

/// Что делает: order-id по своему client-order-id, когда итог постановки неизвестен: поиск; не найден — отмена по
/// client-order-id и поиск с повторами до venue.order_final_wait_ms. Возвращает: order-id; None — не найден.
pub async fn recover(ctx: &Ctx, client_oid: &str) -> Option<String> {
    let id = Id::Client(client_oid);
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

/// Поле детали ордера в одном из двух написаний (документация — filled-*, SDK — field-*). Ошибка BadResponse.
pub fn filled(s: &Value, what: &str) -> GatewayResult<Option<Decimal>> {
    match num::dec_opt(s, &format!("filled-{what}"))? {
        Some(v) => Ok(Some(v)),
        None => num::dec_opt(s, &format!("field-{what}")),
    }
}

/// Что делает: результат ядра по окончательному состоянию ордера и сделкам (matchresults, order_fee.rs).
/// Принимает: запрос ядра, деталь ордера, order-id, получаемую монету. Ошибки: BadResponse — нет полей;
/// OutcomeUnknown — исполнено, а комиссия не подтверждена.
pub async fn result(
    ctx: &Ctx,
    req: TradeRequest,
    s: &Value,
    order_id: String,
    received: &str,
) -> GatewayResult<TradeResult> {
    let size = num::dec(s, "amount")?;
    let qty = filled(s, "amount")?.unwrap_or(Decimal::ZERO);
    let funds = filled(s, "cash-amount")?.unwrap_or(Decimal::ZERO);
    let (fee, fee_asset) = if qty.is_zero() {
        (Decimal::ZERO, received.to_string())
    } else {
        order_fee::fact(ctx, &order_id, received)
            .await
            .map_err(|why| unknown(ctx, &order_id, &why))?
    };
    let status = if qty.is_zero() {
        TradeStatus::NotFilled
    } else if qty < size {
        TradeStatus::PartiallyFilled
    } else {
        TradeStatus::Filled
    };
    Ok(TradeResult {
        request: req,
        status,
        filled_qty: qty,
        avg_price: if qty.is_zero() {
            Decimal::ZERO
        } else {
            funds / qty
        },
        fees: Fee::single(fee, fee_asset),
        venue_ref: order_id,
        chain: None,
        finished_at: Utc::now(),
        executor: None,
    })
}

/// Отмена по order-id или client-order-id. Ошибки: коды HTX, связь.
async fn cancel(ctx: &Ctx, id: Id<'_>) -> GatewayResult<Value> {
    match id {
        Id::Order(o) => {
            let path = format!("/v1/order/orders/{o}/submitcancel");
            rest::signed(ctx, Method::POST, &path, vec![], None, Op::Trade).await
        }
        Id::Client(c) => {
            let path = "/v1/order/orders/submitCancelClientOrder";
            let body = json!({ "client-order-id": c });
            rest::signed(ctx, Method::POST, path, vec![], Some(body), Op::Trade).await
        }
    }
}

/// Деталь ордера (data — объект). Ошибки: как у rest::get; BadResponse — ордера нет в ответе.
pub async fn query(ctx: &Ctx, id: Id<'_>) -> GatewayResult<Value> {
    let data = match id {
        Id::Order(o) => rest::get(ctx, &format!("/v1/order/orders/{o}"), vec![]).await?,
        Id::Client(c) => {
            let q = vec![("clientOrderId", c.to_string())];
            rest::get(ctx, "/v1/order/orders/getClientOrder", q).await?
        }
    };
    if data.is_object() {
        return Ok(data);
    }
    Err(super::errors::bad(
        "деталь ордера: data не объект",
        &data.to_string(),
    ))
}

/// «Ордер отправлен, итог не получен» — OutcomeUnknown (решение 64).
fn unknown(ctx: &Ctx, order_id: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        format!("Ордер HTX {order_id} отправлен, но окончательное состояние не получено: {why}"),
    ))
}
