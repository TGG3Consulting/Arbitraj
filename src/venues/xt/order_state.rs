//! Файл: venues/xt/order_state.rs
//! Что делает: доводит ордер XT.com до окончательного состояния и собирает итог ядра. Ответ POST /v4/order содержит
//! только orderId — итог всегда запросом GET /v4/order/{orderId}: опрос до окончательного состояния; сбой запроса —
//! повтор до срока; не дождался за venue.order_final_wait_ms — отмена (правило Тиграна «не исполнился — отменяем») и
//! ещё опрос. Ордер с неизвестным итогом постановки ищется по своему clientOrderId (GET /v4/order?clientOrderId=) с
//! повторами до срока; отмены по clientOrderId в документации нет — не найден, значит «итог неизвестен».
//! Комиссия: при списании комиссии в XT — из deductServices («if set XT deduction fee and the deduction occurs, use
//! this field»), иначе fee и feeCurrency.
//! Кто вызывает: trade.rs, order_rules.rs (times — проверка до отправки).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.order_poll_ms, venue.order_final_wait_ms.
//!
//! Официально: состояния — publicModule.md «Order state»: NEW, PARTIALLY_FILLED, FILLED, CANCELED, REJECTED, EXPIRED;
//! GET /v4/order/{orderId} (#orderorderGet), GET /v4/order?orderId=|clientOrderId= (#orderorderGetQueryParam), DELETE
//! /v4/order/{orderId} (#orderorderDel) → {cancelId}; поля origQty, executedQty, leavingQty, avgPrice, fee, feeCurrency,
//! deductServices[{fee, feeCurrency}], state. Каким станет IOC с остатком (CANCELED или EXPIRED) — не сказано: оба
//! окончательные (проверить на живом ордере, урок 30).

use std::time::{Duration, Instant};

use chrono::Utc;
use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Fee, TradeRequest, TradeResult, TradeStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::{Ctx, ticker};
use super::errors::Op;
use super::{num, rest};

/// Окончательные состояния ордера (publicModule.md).
const FINAL: [&str; 4] = ["FILLED", "CANCELED", "REJECTED", "EXPIRED"];

/// Сроки ожидания: venue.order_poll_ms и venue.order_final_wait_ms. Ошибка NotConfigured — не задана.
pub fn times(ctx: &Ctx) -> GatewayResult<(Duration, Duration)> {
    Ok((
        Duration::from_millis(ctx.int("venue.order_poll_ms")?),
        Duration::from_millis(ctx.int("venue.order_final_wait_ms")?),
    ))
}

/// Состояние окончательное. Ошибок нет.
pub fn is_final(s: &Value) -> bool {
    num::text_opt(s, "state").is_some_and(|st| FINAL.contains(&st.as_str()))
}

/// Что делает: окончательное состояние ордера по orderId. Возвращает: ответ XT с окончательным state.
/// Ошибки: OutcomeUnknown — не получено и после отмены.
pub async fn settle(ctx: &Ctx, order_id: &str) -> GatewayResult<Value> {
    let (poll, wait) = times(ctx)?;
    if let Ok(state) = poll_final(ctx, order_id, poll, wait).await {
        return Ok(state);
    }
    let path = format!("/v4/order/{order_id}");
    let cancel = rest::signed(ctx, Method::DELETE, &path, vec![], None, Op::Trade).await;
    match poll_final(ctx, order_id, poll, wait).await {
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

/// Что делает: orderId по своему clientOrderId, когда итог постановки неизвестен: поиск с повторами до
/// venue.order_final_wait_ms. Возвращает: orderId; None — не найден. Ошибок нет (сбои — None).
pub async fn recover(ctx: &Ctx, client_id: &str) -> Option<String> {
    let (poll, wait) = times(ctx).ok()?;
    let started = Instant::now();
    loop {
        let q = vec![("clientOrderId", client_id.to_string())];
        if let Ok(s) = rest::signed(ctx, Method::GET, "/v4/order", q, None, Op::Other).await
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
    order_id: &str,
    poll: Duration,
    wait: Duration,
) -> Result<Value, String> {
    let started = Instant::now();
    let path = format!("/v4/order/{order_id}");
    loop {
        let why = match rest::signed(ctx, Method::GET, &path, vec![], None, Op::Other).await {
            Ok(s) if is_final(&s) => return Ok(s),
            Ok(s) => format!("state {}", num::text_opt(&s, "state").unwrap_or_default()),
            Err(e) => format!("запрос состояния: {}", e.message),
        };
        if started.elapsed() >= wait {
            return Err(why);
        }
        tokio::time::sleep(poll).await;
    }
}

/// Что делает: комиссия ордера и её монета (тикер ядра). deductServices непустой — из него, иначе fee/feeCurrency.
/// Принимает: состояние ордера. Возвращает: (комиссия, монета; пусто — комиссии нет).
/// Ошибка BadResponse — комиссия в разных монетах (сверить вручную) или поле не число.
pub fn fee(s: &Value) -> GatewayResult<(Decimal, String)> {
    let list = s
        .get("deductServices")
        .and_then(Value::as_array)
        .filter(|l| !l.is_empty());
    let Some(list) = list else {
        let f = num::dec_opt(s, "fee")?.unwrap_or_default().abs();
        let asset = num::text_opt(s, "feeCurrency")
            .map(|c| ticker(&c))
            .unwrap_or_default();
        return Ok((f, asset));
    };
    let (mut total, mut asset) = (Decimal::ZERO, String::new());
    for d in list {
        let a = num::text_opt(d, "feeCurrency")
            .map(|c| ticker(&c))
            .unwrap_or_default();
        let f = num::dec_opt(d, "fee")?.unwrap_or_default().abs();
        if f.is_zero() {
            continue;
        }
        if !asset.is_empty() && a != asset {
            return Err(super::errors::bad(
                &format!(
                    "ордер {}: комиссия в разных монетах ({asset}, {a}) — сверить вручную",
                    num::text_opt(s, "orderId").unwrap_or_default()
                ),
                &s.to_string(),
            ));
        }
        asset = a;
        total += f;
    }
    Ok((total, asset))
}

/// Что делает: итог ядра по окончательному состоянию. Принимает: запрос ядра, состояние, orderId, отправленное
/// количество (если нет origQty). Ошибки: BadResponse — нет полей или комиссия в разных монетах.
pub fn result(
    req: TradeRequest,
    s: &Value,
    order_id: String,
    sent_qty: Decimal,
) -> GatewayResult<TradeResult> {
    let orig = num::dec_opt(s, "origQty")?.unwrap_or(sent_qty);
    let filled = num::dec(s, "executedQty")?;
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
        match num::dec_opt(s, "avgPrice")?.filter(|p| *p > Decimal::ZERO) {
            Some(p) => p,
            None => num::dec(s, "tradeQuote")? / filled,
        }
    };
    let (fee, fee_asset) = if filled.is_zero() {
        (Decimal::ZERO, String::new())
    } else {
        fee(s)?
    };
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

/// «Ордер отправлен, итог не получен» — OutcomeUnknown (решение 64). Ошибок нет.
fn unknown(ctx: &Ctx, order_id: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        format!("Ордер {order_id} отправлен, но окончательное состояние не получено: {why}"),
    ))
}
