//! Файл: venues/bitget/order_state.rs
//! Что делает: доводит ордер Bitget до окончательного состояния (filled / cancelled): опрос по orderId (сбой запроса —
//! повтор до срока); не дождался за venue.order_final_wait_ms — отмена остатка (правило Тиграна) и чтение итога ещё
//! venue.order_final_wait_ms. Ордер с неизвестным итогом постановки ищется по своему clientOid; не найден — отмена по
//! clientOid и снова поиск. Итог: baseVolume, quoteVolume, комиссия и её монета (order_fee.rs) самого ордера.
//! Кто вызывает: trade.rs, order_rules.rs (times — проверка до постановки).
//! От чего зависит: rest.rs, num.rs, order_fee.rs, ctx.rs.
//! Настройки: venue.order_poll_ms, venue.order_final_wait_ms [bitget].
//!
//! Официально (https://www.bitget.com/docs/catalog/classic-spot-trade/classic-spot-trade):
//! - Get Order Info GET /api/v2/spot/trade/orderInfo?orderId= | clientOid= («20 times/1s (UID)») — data: массив;
//!   status «live», «partially_filled», «filled», «cancelled»; priceAvg, size, baseVolume «Filled quantity (base
//!   coin)», quoteVolume «Total trading amount (quote coin)», baseCoin, quoteCoin, feeDetail (строка JSON).
//!   Противоречие: перечень пишет «cancelled», Best Practices — «live → canceled»; принимаются оба (урок 2, проверить).
//! - Cancel Order POST /api/v2/spot/trade/cancel-order — symbol, orderId или clientOid.

use std::time::{Duration, Instant};

use chrono::Utc;
use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::{TradeRequest, TradeResult, TradeStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, order_fee, rest};

/// Окончательные статусы ордера (перечень документации и написание Best Practices).
const FINAL: [&str; 3] = ["filled", "cancelled", "canceled"];

/// Как искать ордер: по orderId биржи или по своему clientOid.
#[derive(Clone, Copy)]
pub enum Id<'a> {
    Order(&'a str),
    Client(&'a str),
}

impl Id<'_> {
    /// Параметр запроса: («orderId» | «clientOid», значение). Ошибок нет.
    fn param(self) -> (&'static str, String) {
        match self {
            Id::Order(o) => ("orderId", o.to_string()),
            Id::Client(c) => ("clientOid", c.to_string()),
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

/// Что делает: окончательное состояние ордера. Принимает: символ, orderId.
/// Ошибки: OutcomeUnknown — не получено даже после отмены; NotConfigured — сроки.
pub async fn settle(ctx: &Ctx, symbol: &str, order_id: &str) -> GatewayResult<Value> {
    let (poll, wait) = times(ctx)?;
    let id = Id::Order(order_id);
    if let Ok(s) = poll_final(ctx, id, poll, wait).await {
        return Ok(s);
    }
    let cancel = cancel(ctx, symbol, id).await;
    poll_final(ctx, id, poll, wait).await.map_err(|why| {
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
    let found = |s: GatewayResult<Value>| s.ok().and_then(|s| num::text_opt(&s, "orderId"));
    if let Some(o) = found(query(ctx, id).await) {
        return Some(o);
    }
    let (poll, wait) = times(ctx).ok()?;
    let _ = cancel(ctx, symbol, id).await;
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

/// Опрос до окончательного статуса не дольше wait; сбой запроса — повтор. Ошибка — текст последней причины.
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
                let st = num::text_opt(&s, "status").unwrap_or_default();
                if FINAL.contains(&st.as_str()) {
                    return Ok(s);
                }
                format!("ордер ещё не закончен (status = {st})")
            }
            Err(e) => format!("запрос ордера: {}", e.message),
        };
        if started.elapsed() >= wait {
            return Err(why);
        }
        tokio::time::sleep(poll).await;
    }
}

/// Что делает: результат ядра по окончательному состоянию ордера.
/// Принимает: запрос ядра, состояние, orderId, получаемую монету сделки (если в ответе нет baseCoin/quoteCoin).
/// Ошибки: BadResponse — нет полей; OutcomeUnknown — исполнено, а комиссия не подтверждена (order_fee.rs).
pub fn result(
    ctx: &Ctx,
    req: TradeRequest,
    s: &Value,
    order_id: String,
    received: &str,
) -> GatewayResult<TradeResult> {
    let size = num::dec(s, "size")?;
    let filled = num::dec_opt(s, "baseVolume")?.unwrap_or(Decimal::ZERO);
    let funds = num::dec_opt(s, "quoteVolume")?.unwrap_or(Decimal::ZERO);
    let field = if req.side == crate::core::types::Side::Buy {
        "baseCoin"
    } else {
        "quoteCoin"
    };
    let coin = num::text_opt(s, field)
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or_else(|| received.to_string());
    let (fee, fee_asset) = match order_fee::parse(s.get("feeDetail"), &coin) {
        Ok(f) => f,
        Err(why) if !filled.is_zero() => return Err(unknown(ctx, &order_id, &why)),
        Err(_) => (Decimal::ZERO, coin),
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
        fee,
        fee_asset,
        venue_ref: order_id,
        chain: None,
        finished_at: Utc::now(),
        executor: None,
    })
}

/// Отмена по orderId или clientOid. Ошибки: коды Bitget, связь.
async fn cancel(ctx: &Ctx, symbol: &str, id: Id<'_>) -> GatewayResult<Value> {
    let (k, v) = id.param();
    let mut body = json!({ "symbol": symbol });
    body[k] = Value::String(v);
    let path = "/api/v2/spot/trade/cancel-order";
    rest::signed(ctx, Method::POST, path, vec![], Some(body), Op::Trade).await
}

/// Запрос ордера: первый элемент массива data. Ошибки: как у rest::signed; BadResponse — ордера нет в ответе.
pub async fn query(ctx: &Ctx, id: Id<'_>) -> GatewayResult<Value> {
    let path = "/api/v2/spot/trade/orderInfo";
    let data = rest::signed(ctx, Method::GET, path, vec![id.param()], None, Op::Other).await?;
    match data {
        Value::Array(a) if !a.is_empty() => Ok(a.into_iter().next().unwrap_or(Value::Null)),
        other => Err(super::errors::bad(
            "orderInfo: ордера нет в ответе (data пуст или не массив)",
            &other.to_string(),
        )),
    }
}

/// «Ордер отправлен, итог не получен» — OutcomeUnknown (решение 64).
fn unknown(ctx: &Ctx, order_id: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        format!("Ордер Bitget {order_id} отправлен, но окончательное состояние не получено: {why}"),
    ))
}
