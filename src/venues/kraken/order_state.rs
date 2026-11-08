//! Файл: venues/kraken/order_state.rs
//! Что делает: доводит ордер Kraken до окончательного состояния (closed, canceled, expired): опрос QueryOrders по txid
//! (сбой запроса — повтор до срока); не дождался за venue.order_final_wait_ms — отмена остатка (правило Тиграна) и чтение
//! итога ещё venue.order_final_wait_ms. Ордер с неизвестным итогом постановки — отмена по своему cl_ord_id и поиск по
//! cl_ord_id среди открытых и закрытых. Итог: vol_exec, price (средняя), fee (в котируемой — oflags fciq).
//! Кто вызывает: trade.rs, order_rules.rs (times — проверка до постановки).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.order_poll_ms, venue.order_final_wait_ms, venue.quote_asset [kraken].
//!
//! Официально (OpenAPI spot-rest.yaml): POST /0/private/QueryOrders — txid (через запятую), result «txid → order»; схема
//! open/order: status pending, open, closed, canceled, expired; vol, vol_exec «Volume executed (base currency)», cost,
//! fee «Total fee (quote currency)», price «Average price», cl_ord_id. POST /0/private/CancelOrder — txid или cl_ord_id.
//! OpenOrders → result.open, ClosedOrders → result.closed («txid → order»). Поиск по cl_ord_id в QueryOrders в
//! прочитанном не подтверждён — ищем по полю cl_ord_id ордеров списков (есть ли оно в ответе — проверить живьём;
//! нет — итог неизвестен, ручная проверка). Остаток IOC — status expired (проверить пробным/живым ордером, урок 30).

use std::time::{Duration, Instant};

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Fee, TradeRequest, TradeResult, TradeStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::num;
use super::rest::{self, p};

/// Окончательные состояния ордера (перечень status документации).
const FINAL: [&str; 3] = ["closed", "canceled", "expired"];

/// Сроки ожидания: venue.order_poll_ms и venue.order_final_wait_ms. Ошибка NotConfigured — не задана.
pub fn times(ctx: &Ctx) -> GatewayResult<(Duration, Duration)> {
    Ok((
        Duration::from_millis(ctx.int("venue.order_poll_ms")?),
        Duration::from_millis(ctx.int("venue.order_final_wait_ms")?),
    ))
}

/// Что делает: окончательное состояние ордера. Принимает: txid. Ошибки: OutcomeUnknown — не получено даже после
/// отмены; NotConfigured — сроки.
pub async fn settle(ctx: &Ctx, txid: &str) -> GatewayResult<Value> {
    let (poll, wait) = times(ctx)?;
    if let Ok(s) = poll_final(ctx, txid, poll, wait).await {
        return Ok(s);
    }
    let cancel = rest::private(ctx, "CancelOrder", vec![p("txid", txid)], Op::Other).await;
    poll_final(ctx, txid, poll, wait).await.map_err(|why| {
        let c = cancel
            .err()
            .map(|e| format!("отмена: {}; ", e.message))
            .unwrap_or_default();
        unknown(ctx, txid, &format!("{c}{why}"))
    })
}

/// Что делает: txid по своему cl_ord_id, когда итог постановки неизвестен: отмена по cl_ord_id, затем поиск среди
/// открытых и закрытых с повторами до venue.order_final_wait_ms. Возвращает: txid; None — не найден. Ошибок нет.
pub async fn recover(ctx: &Ctx, cl_ord_id: &str) -> Option<String> {
    let (poll, wait) = times(ctx).ok()?;
    let _ = rest::private(
        ctx,
        "CancelOrder",
        vec![p("cl_ord_id", cl_ord_id)],
        Op::Other,
    )
    .await;
    let started = Instant::now();
    loop {
        for (method, field) in [("OpenOrders", "open"), ("ClosedOrders", "closed")] {
            if let Ok(res) = rest::private(ctx, method, vec![], Op::Other).await
                && let Some(id) = by_client(res.get(field), cl_ord_id)
            {
                return Some(id);
            }
        }
        if started.elapsed() >= wait {
            return None;
        }
        tokio::time::sleep(poll).await;
    }
}

/// txid ордера с нашим cl_ord_id в объекте «txid → order». Ошибок нет.
pub fn by_client(orders: Option<&Value>, cl_ord_id: &str) -> Option<String> {
    orders?
        .as_object()?
        .iter()
        .find(|(_, o)| num::text_opt(o, "cl_ord_id").as_deref() == Some(cl_ord_id))
        .map(|(id, _)| id.clone())
}

/// Опрос до окончательного status не дольше wait; сбой запроса — повтор. Ошибка — текст последней причины.
async fn poll_final(
    ctx: &Ctx,
    txid: &str,
    poll: Duration,
    wait: Duration,
) -> Result<Value, String> {
    let started = Instant::now();
    loop {
        let why = match rest::private(ctx, "QueryOrders", vec![p("txid", txid)], Op::Other).await {
            Ok(mut res) => match res.as_object_mut().and_then(|m| m.remove(txid)) {
                Some(o) => match num::text_opt(&o, "status") {
                    Some(st) if FINAL.contains(&st.as_str()) => return Ok(o),
                    st => format!("ордер ещё не закончен (status {st:?})"),
                },
                None => format!("в ответе QueryOrders нет {txid}"),
            },
            Err(e) => format!("запрос ордера: {}", e.message),
        };
        if started.elapsed() >= wait {
            return Err(why);
        }
        tokio::time::sleep(poll).await;
    }
}

/// Что делает: результат ядра по окончательному состоянию (vol, vol_exec, price, fee).
/// Принимает: окружение, запрос ядра, состояние, txid. Ошибки: BadResponse — нет полей; OutcomeUnknown — исполнено, а
/// средней цены нет; NotConfigured — venue.quote_asset.
pub fn result(ctx: &Ctx, req: TradeRequest, s: &Value, txid: String) -> GatewayResult<TradeResult> {
    let size = num::dec(s, "vol")?;
    let filled = num::dec_opt(s, "vol_exec")?.unwrap_or(Decimal::ZERO);
    let avg = num::dec_opt(s, "price")?.unwrap_or(Decimal::ZERO);
    let fee = num::dec_opt(s, "fee")?.unwrap_or(Decimal::ZERO);
    if !filled.is_zero() && avg.is_zero() {
        return Err(unknown(
            ctx,
            &txid,
            "исполнено, а средней цены price в ответе нет",
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
        // «Total fee (quote currency)» и oflags fciq — монета комиссии котируемая.
        fees: Fee::single(fee, ctx.text("venue.quote_asset")?),
        venue_ref: txid,
        chain: None,
        finished_at: Utc::now(),
        executor: None,
    })
}

/// «Ордер отправлен, итог не получен» — OutcomeUnknown (решение 64).
fn unknown(ctx: &Ctx, txid: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        format!("Ордер Kraken {txid} отправлен, но окончательное состояние не получено: {why}"),
    ))
}
