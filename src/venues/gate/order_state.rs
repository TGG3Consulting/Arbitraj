//! Файл: venues/gate/order_state.rs
//! Что делает: доводит ордер Gate до окончательного состояния (closed / cancelled): опрашивает его; сбой запроса —
//! повтор до срока (как MEXC, решение 118, А-🔴2); не дождался за venue.order_final_wait_ms — отменяет остаток
//! (правило Тиграна «не исполнился — отменяем») и читает итог с повторами ещё столько же. Ордер, итог постановки
//! которого неизвестен, ищется по своему text: среди ожидающих, затем отмена по text и поиск в истории законченных
//! (А-🟠4). Итог — из полей ордера: filled_amount, filled_total, fee, fee_currency (и gt_fee / point_fee).
//! Кто вызывает: trade.rs, order_rules.rs (times, history_limit — проверка до отправки).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs.
//! Настройки: venue.order_poll_ms, venue.order_final_wait_ms, venue.order_history_limit [gate].
//!
//! Официальная документация https://www.gate.com/docs/developers/apiv4/en/ (SDK docs/SpotApi.md, docs/Order.md):
//! - GET /spot/orders/{order_id}?currency_pair= и DELETE /spot/orders/{order_id}?currency_pair=: order_id — id или text;
//!   «Operations based on custom IDs can only be checked in pending orders. Only order ID can be used after the order is
//!   finished»; status — open, closed («ended»), cancelled; «If the order is cancelled before being fully filled, its
//!   status becomes cancelled regardless of whether it was partially filled».
//! - GET /spot/orders?currency_pair=&status=finished&from=&limit= — история законченных ордеров (поиск по text).
//! - Поля итога: filled_amount, filled_total («Total filled in quote currency»), fee, fee_currency, gt_fee («GT used to
//!   deduct fee»), point_fee («Points used to deduct fee»), finish_as.

use std::time::{Duration, Instant};

use chrono::Utc;
use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{TradeRequest, TradeResult, TradeStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{num, rest};

/// Окончательные состояния ордера по документации.
const FINAL: [&str; 2] = ["closed", "cancelled"];

/// Сроки ожидания: venue.order_poll_ms и venue.order_final_wait_ms. Ошибка NotConfigured — не задана.
pub fn times(ctx: &Ctx) -> GatewayResult<(Duration, Duration)> {
    Ok((
        Duration::from_millis(ctx.int("venue.order_poll_ms")?),
        Duration::from_millis(ctx.int("venue.order_final_wait_ms")?),
    ))
}

/// Сколько законченных ордеров читать при поиске по text (venue.order_history_limit; в документации — limit, «default
/// 100»). Ошибка NotConfigured — не задана.
pub fn history_limit(ctx: &Ctx) -> GatewayResult<u64> {
    ctx.int("venue.order_history_limit")
}

/// Состояние окончательное. Ошибок нет.
pub fn is_final(s: &Value) -> bool {
    num::text_opt(s, "status").is_some_and(|st| FINAL.contains(&st.as_str()))
}

/// Что делает: окончательное состояние ордера по id. Ошибки: OutcomeUnknown — не получено даже после отмены.
pub async fn settle(ctx: &Ctx, symbol: &str, order_id: &str) -> GatewayResult<Value> {
    let (poll, wait) = times(ctx)?;
    if let Ok(s) = poll_final(ctx, symbol, order_id, poll, wait).await {
        return Ok(s);
    }
    let cancel = call(ctx, Method::DELETE, symbol, order_id, Op::Trade).await;
    match poll_final(ctx, symbol, order_id, poll, wait).await {
        Ok(s) => Ok(s),
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

/// Что делает: id ордера по своему text, когда итог постановки неизвестен (см. шапку). Принимает: окружение, символ,
/// text, когда отправлен (Unix с). Возвращает: id ордера; None — не найден (итог неизвестен). Ошибок нет.
pub async fn recover(ctx: &Ctx, symbol: &str, text: &str, sent_s: i64) -> Option<String> {
    if let Ok(s) = call(ctx, Method::GET, symbol, text, Op::Other).await
        && let Some(id) = num::text_opt(&s, "id")
    {
        return Some(id);
    }
    let (poll, wait) = times(ctx).ok()?;
    let limit = history_limit(ctx).ok()?;
    let _ = call(ctx, Method::DELETE, symbol, text, Op::Trade).await;
    let started = Instant::now();
    loop {
        let p = vec![
            ("currency_pair", symbol.to_string()),
            ("status", "finished".to_string()),
            ("from", sent_s.to_string()),
            ("limit", limit.to_string()),
        ];
        if let Ok(Value::Array(list)) =
            rest::signed(ctx, Method::GET, "/spot/orders", p, None, Op::Other).await
            && let Some(id) = list
                .iter()
                .find(|o| num::text_opt(o, "text").as_deref() == Some(text))
                .and_then(|o| num::text_opt(o, "id"))
        {
            return Some(id);
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
    id: &str,
    poll: Duration,
    wait: Duration,
) -> Result<Value, String> {
    let started = Instant::now();
    loop {
        let why = match call(ctx, Method::GET, symbol, id, Op::Other).await {
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

/// Что делает: результат ядра по окончательному состоянию (поля — шапка). Комиссия скидкой GT — монета GT; в двух
/// монетах сразу или очками — BadResponse (факт есть, но комиссия не в одной монете: ручная проверка, как MEXC).
/// Принимает: запрос ядра, состояние, id, отправленное количество (если нет amount). Ошибки: BadResponse.
pub fn result(
    ctx: &Ctx,
    req: TradeRequest,
    s: &Value,
    order_id: String,
    sent: Decimal,
) -> GatewayResult<TradeResult> {
    let orig = num::dec_opt(s, "amount")?.unwrap_or(sent);
    let filled = num::dec(s, "filled_amount")?;
    let quote = num::dec_opt(s, "filled_total")?
        .or(num::dec_opt(s, "fill_price")?)
        .ok_or_else(|| errors::bad("нет filled_total", &s.to_string()))?;
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
    let zero = |f: &str| num::dec_opt(s, f).map(|v| v.unwrap_or_default().abs());
    let (fee, gt, point) = (zero("fee")?, zero("gt_fee")?, zero("point_fee")?);
    let (fee, fee_asset) = match (fee.is_zero(), gt.is_zero(), point.is_zero()) {
        (_, true, true) => (fee, num::text_opt(s, "fee_currency").unwrap_or_default()),
        (true, false, true) => (gt, "GT".to_string()),
        _ => {
            let m = format!(
                "ордер {order_id}: комиссия не в одной монете (fee {fee}, gt_fee {gt}, point_fee {point})"
            );
            return Err(ctx.tag(errors::bad(&m, &s.to_string())));
        }
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
        executor: None,
    })
}

/// Запрос или отмена ордера по id или text. Ошибки: как у rest::signed (вызывающий решает, что делать).
async fn call(ctx: &Ctx, m: Method, symbol: &str, id: &str, op: Op) -> GatewayResult<Value> {
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    {
        return Err(errors::bad("ID ордера с недопустимыми знаками", id));
    }
    let q = vec![("currency_pair", symbol.to_string())];
    rest::signed(ctx, m, &format!("/spot/orders/{id}"), q, None, op).await
}

/// «Ордер отправлен, итог не получен» — OutcomeUnknown (решение 64).
fn unknown(ctx: &Ctx, order_id: &str, why: &str) -> GatewayError {
    let m = format!("Ордер {order_id} отправлен, но окончательное состояние не получено: {why}");
    ctx.tag(GatewayError::new(ErrorCode::OutcomeUnknown, None, m))
}
