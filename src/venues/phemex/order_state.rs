//! Файл: venues/phemex/order_state.rs
//! Что делает: доводит ордер Phemex до окончательного состояния (Filled / Canceled / Rejected): опрос по своему clOrdID
//! (сбой запроса или ордер ещё не виден — повтор до срока); не дождался за venue.order_final_wait_ms — отмена остатка
//! по clOrdID (правило Тиграна) и чтение итога ещё venue.order_final_wait_ms; не получен — «итог неизвестен». Итог:
//! исполненное количество, сумма, комиссия и её монета самого ордера.
//! Кто вызывает: trade.rs, order_rules.rs (times — проверка до постановки).
//! От чего зависит: rest.rs, num.rs, order_rules.rs (Prepared), ctx.rs.
//! Настройки: venue.order_poll_ms, venue.order_final_wait_ms [phemex].
//!
//! Официально (Spot REST API, https://phemex-docs.github.io/):
//! - Query orders by order ID or client order ID GET /api-data/spots/orders/by-order-id?symbol=&clOrdID=
//!   (#query-orders-by-order-id-or-client-order-id) — массив: ordStatus, baseQtyEv, cumBaseValueEv, cumQuoteValueEv,
//!   cumFeeEv, feeCurrency, avgTransactPriceEp, orderID. В строке запроса параметр orderID написан «oderId» (опечатка) —
//!   ищем только по clOrdID (он есть всегда). Смысл cumBaseValueEv / cumQuoteValueEv (исполнено в базовой / сумма в
//!   котируемой) описанием полей не подтверждён — проверить на первом живом ордере (урок 7).
//! - Order status: Rejected, New, PartiallyFilled, Filled, Canceled (Common order fields); у ответа создания —
//!   «Created». Окончательные — Filled, Canceled, Rejected.
//! - Cancel order DELETE /spot/orders?symbol=&clOrdID= (#cancel-order).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use chrono::Utc;
use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{TradeRequest, TradeResult, TradeStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::order_rules::Prepared;
use super::{num, rest};

/// Окончательные статусы ордера (перечень Order status).
const FINAL: [&str; 3] = ["Filled", "Canceled", "Rejected"];

/// Сроки ожидания: venue.order_poll_ms и venue.order_final_wait_ms. Ошибка NotConfigured — не задана.
pub fn times(ctx: &Ctx) -> GatewayResult<(Duration, Duration)> {
    Ok((
        Duration::from_millis(ctx.int("venue.order_poll_ms")?),
        Duration::from_millis(ctx.int("venue.order_final_wait_ms")?),
    ))
}

/// Что делает: окончательное состояние ордера по clOrdID. Принимает: подготовленный ордер.
/// Ошибки: OutcomeUnknown — не получено даже после отмены; NotConfigured — сроки.
pub async fn settle(ctx: &Ctx, p: &Prepared) -> GatewayResult<Value> {
    let (poll, wait) = times(ctx)?;
    if let Ok(s) = poll_final(ctx, p, poll, wait).await {
        return Ok(s);
    }
    let cancel = cancel(ctx, p).await;
    poll_final(ctx, p, poll, wait).await.map_err(|why| {
        let c = cancel
            .err()
            .map(|e| format!("отмена: {}; ", e.message))
            .unwrap_or_default();
        unknown(ctx, &p.cl_ord_id, &format!("{c}{why}"))
    })
}

/// Опрос до окончательного статуса не дольше wait; сбой запроса — повтор. Ошибка — текст последней причины.
async fn poll_final(
    ctx: &Ctx,
    p: &Prepared,
    poll: Duration,
    wait: Duration,
) -> Result<Value, String> {
    let started = Instant::now();
    loop {
        let why = match query(ctx, &p.symbol, &p.cl_ord_id).await {
            Ok(Some(s)) => {
                let st = num::text_opt(&s, "ordStatus").unwrap_or_default();
                if FINAL.contains(&st.as_str()) {
                    return Ok(s);
                }
                format!("ордер ещё не закончен (ordStatus = {st})")
            }
            Ok(None) => "ордер ещё не виден в by-order-id".to_string(),
            Err(e) => format!("запрос ордера: {}", e.message),
        };
        if started.elapsed() >= wait {
            return Err(why);
        }
        tokio::time::sleep(poll).await;
    }
}

/// Что делает: результат ядра по окончательному состоянию ордера.
/// Принимает: запрос ядра, состояние, подготовленный ордер, масштабы монет (комиссия — в масштабе feeCurrency).
/// Ошибки: BadResponse — поле не целое; OutcomeUnknown — исполнено, а комиссия не прочитана (нет монеты или масштаба).
pub fn result(
    ctx: &Ctx,
    req: TradeRequest,
    s: &Value,
    p: &Prepared,
    scales: &HashMap<String, u32>,
) -> GatewayResult<TradeResult> {
    let filled = num::scaled_opt(s, "cumBaseValueEv", p.base_scale)?.unwrap_or(Decimal::ZERO);
    let funds = num::scaled_opt(s, "cumQuoteValueEv", p.quote_scale)?.unwrap_or(Decimal::ZERO);
    let order_id = num::text_opt(s, "orderID").unwrap_or_else(|| p.cl_ord_id.clone());
    let coin = num::text_opt(s, "feeCurrency");
    let fee = match coin.as_ref().and_then(|c| scales.get(c)) {
        Some(sc) => num::scaled_opt(s, "cumFeeEv", *sc)?
            .unwrap_or(Decimal::ZERO)
            .abs(),
        None if filled.is_zero() => Decimal::ZERO,
        None => {
            let why =
                format!("комиссия не прочитана: feeCurrency {coin:?} без valueScale в справочнике");
            return Err(unknown(ctx, &p.cl_ord_id, &why));
        }
    };
    let status = if filled.is_zero() {
        TradeStatus::NotFilled
    } else if filled < p.qty {
        TradeStatus::PartiallyFilled
    } else {
        TradeStatus::Filled
    };
    let fee_asset = coin.unwrap_or_else(|| p.received.clone());
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

/// Отмена по clOrdID. Ошибки: коды Phemex, связь.
async fn cancel(ctx: &Ctx, p: &Prepared) -> GatewayResult<Value> {
    let q = vec![
        ("symbol", p.symbol.clone()),
        ("clOrdID", p.cl_ord_id.clone()),
    ];
    rest::signed(ctx, Method::DELETE, "/spot/orders", q, None, Op::Trade).await
}

/// Запрос ордера по clOrdID: первый элемент ответа. Возвращает: None — ордера в ответе нет. Ошибки: как rest::signed.
pub async fn query(ctx: &Ctx, symbol: &str, cl_ord_id: &str) -> GatewayResult<Option<Value>> {
    let q = vec![
        ("symbol", symbol.to_string()),
        ("clOrdID", cl_ord_id.to_string()),
    ];
    let path = "/api-data/spots/orders/by-order-id";
    let data = rest::signed(ctx, Method::GET, path, q, None, Op::Other).await?;
    Ok(num::rows(data, path)?.into_iter().next())
}

/// «Ордер отправлен, итог не получен» — OutcomeUnknown (решение 64).
fn unknown(ctx: &Ctx, cl_ord_id: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        format!(
            "Ордер Phemex {cl_ord_id} отправлен, но окончательное состояние не получено: {why}"
        ),
    ))
}
