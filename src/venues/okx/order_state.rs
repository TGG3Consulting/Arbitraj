//! Файл: venues/okx/order_state.rs
//! Что делает: доводит ордер OKX до окончательного состояния (filled, canceled, mmp_canceled): опрос по ordId (сбой
//! запроса — повтор до срока); не дождался за venue.order_final_wait_ms — отмена остатка (правило Тиграна) и чтение
//! итога ещё venue.order_final_wait_ms. Ордер с неизвестным итогом постановки ищется по своему clOrdId; не найден —
//! отмена по clOrdId и снова поиск. Итог: accFillSz, avgPx, fee, feeCcy самого ордера.
//! Кто вызывает: trade.rs, order_rules.rs (times — проверка до постановки).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.order_poll_ms, venue.order_final_wait_ms [okx].
//!
//! Официально (https://www.okx.com/docs-v5/en/, «Order Book Trading → Trade»):
//! - «GET / Order details» GET /api/v5/trade/order (60 запросов / 2 с; instId и ordId или clOrdId): state live,
//!   partially_filled, filled, canceled, mmp_canceled; sz; accFillSz («Running total of filled quantity since order
//!   creation»; у спота — базовая монета); avgPx («If none is filled, it will return ""»); fee («Sign convention:
//!   negative = net fee paid to platform; positive = net rebate received from platform»); feeCcy. Отменённые без
//!   исполнения хранятся 2 часа («Order history (last 7 days)») — итог читается сразу после ордера.
//! - «POST / Cancel order» POST /api/v5/trade/cancel-order (instId, ordId или clOrdId): «Cancel order returns with sCode
//!   equal to 0. It is not strictly considered that the order has been canceled» — после отмены итог снова читается.

use std::time::{Duration, Instant};

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::{Fee, TradeRequest, TradeResult, TradeStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Окончательные состояния ордера (перечень state документации).
const FINAL: [&str; 3] = ["filled", "canceled", "mmp_canceled"];

/// Как искать ордер: по ordId биржи или по своему clOrdId.
#[derive(Clone, Copy)]
pub enum Id<'a> {
    Order(&'a str),
    Client(&'a str),
}

impl Id<'_> {
    /// Пара (имя поля, значение) для запроса. Ошибок нет.
    fn field(&self) -> (&'static str, String) {
        match self {
            Id::Order(o) => ("ordId", o.to_string()),
            Id::Client(c) => ("clOrdId", c.to_string()),
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

/// Что делает: окончательное состояние ордера. Принимает: instId, ordId.
/// Ошибки: OutcomeUnknown — не получено даже после отмены; NotConfigured — сроки.
pub async fn settle(ctx: &Ctx, inst: &str, order_id: &str) -> GatewayResult<Value> {
    let (poll, wait) = times(ctx)?;
    let id = Id::Order(order_id);
    if let Ok(s) = poll_final(ctx, inst, id, poll, wait).await {
        return Ok(s);
    }
    let cancel = cancel(ctx, inst, id).await;
    poll_final(ctx, inst, id, poll, wait).await.map_err(|why| {
        let c = cancel
            .err()
            .map(|e| format!("отмена: {}; ", e.message))
            .unwrap_or_default();
        unknown(ctx, order_id, &format!("{c}{why}"))
    })
}

/// Что делает: ordId по своему clOrdId, когда итог постановки неизвестен: поиск; не найден — отмена по clOrdId и поиск
/// с повторами до venue.order_final_wait_ms. Возвращает: ordId; None — не найден. Ошибок нет.
pub async fn recover(ctx: &Ctx, inst: &str, cl_ord_id: &str) -> Option<String> {
    let id = Id::Client(cl_ord_id);
    if let Ok(s) = query(ctx, inst, id).await
        && let Some(o) = num::text_opt(&s, "ordId")
    {
        return Some(o);
    }
    let (poll, wait) = times(ctx).ok()?;
    let _ = cancel(ctx, inst, id).await;
    let started = Instant::now();
    loop {
        if let Ok(s) = query(ctx, inst, id).await
            && let Some(o) = num::text_opt(&s, "ordId")
        {
            return Some(o);
        }
        if started.elapsed() >= wait {
            return None;
        }
        tokio::time::sleep(poll).await;
    }
}

/// Опрос до окончательного state не дольше wait; сбой запроса — повтор. Ошибка — текст последней причины.
async fn poll_final(
    ctx: &Ctx,
    inst: &str,
    id: Id<'_>,
    poll: Duration,
    wait: Duration,
) -> Result<Value, String> {
    let started = Instant::now();
    loop {
        let why = match query(ctx, inst, id).await {
            Ok(s) => match num::text_opt(&s, "state") {
                Some(st) if FINAL.contains(&st.as_str()) => return Ok(s),
                st => format!("ордер ещё не закончен (state {st:?})"),
            },
            Err(e) => format!("запрос ордера: {}", e.message),
        };
        if started.elapsed() >= wait {
            return Err(why);
        }
        tokio::time::sleep(poll).await;
    }
}

/// Что делает: результат ядра по окончательному состоянию (sz, accFillSz, avgPx, fee, feeCcy).
/// Принимает: запрос ядра, состояние, ordId. Ошибки: BadResponse — нет полей; OutcomeUnknown — исполнено, а монеты
/// комиссии нет (факт не подтверждён).
pub fn result(
    ctx: &Ctx,
    req: TradeRequest,
    s: &Value,
    order_id: String,
) -> GatewayResult<TradeResult> {
    let size = num::dec(s, "sz")?;
    let filled = num::dec_opt(s, "accFillSz")?.unwrap_or(Decimal::ZERO);
    let avg = num::dec_opt(s, "avgPx")?.unwrap_or(Decimal::ZERO);
    // fee: «negative = net fee paid to platform», positive — rebate → списано = −fee.
    let fee = -num::dec_opt(s, "fee")?.unwrap_or(Decimal::ZERO);
    let fee_asset = num::text_opt(s, "feeCcy").unwrap_or_default();
    if !filled.is_zero() && (fee_asset.is_empty() || avg.is_zero()) {
        return Err(unknown(
            ctx,
            &order_id,
            "исполнено, а feeCcy или avgPx в ответе нет",
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
        avg_price: if filled.is_zero() { Decimal::ZERO } else { avg },
        fees: Fee::single(fee, fee_asset),
        venue_ref: order_id,
        chain: None,
        finished_at: Utc::now(),
        executor: None,
    })
}

/// Отмена. Ошибки: коды OKX (51400 — уже исполнен или отменён), связь.
async fn cancel(ctx: &Ctx, inst: &str, id: Id<'_>) -> GatewayResult<Value> {
    let (k, v) = id.field();
    let body = json!({"instId": inst, k: v});
    rest::post(ctx, "/api/v5/trade/cancel-order", body, Op::Other).await
}

/// Запрос ордера (первая строка data). Ошибки: как у rest::get; 51603 «Order does not exist».
pub async fn query(ctx: &Ctx, inst: &str, id: Id<'_>) -> GatewayResult<Value> {
    let (k, v) = id.field();
    let data = rest::get(
        ctx,
        "/api/v5/trade/order",
        vec![("instId", inst.to_string()), (k, v)],
    )
    .await?;
    rest::first(&data, "trade/order").cloned()
}

/// «Ордер отправлен, итог не получен» — OutcomeUnknown (решение 64).
fn unknown(ctx: &Ctx, order_id: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        format!("Ордер OKX {order_id} отправлен, но окончательное состояние не получено: {why}"),
    ))
}
