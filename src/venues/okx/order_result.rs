//! Файл: venues/okx/order_result.rs
//! Что делает: итог ордера OKX для ядра по окончательному состоянию (sz, accFillSz, avgPx, fee, feeCcy). Решение 264:
//! неисполненный ордер (avgPx = "") — «не исполнился» (TradeStatus::NotFilled), а не ошибка разбора; исполнено, а
//! комиссии, её монеты или средней цены нет — итог неизвестен (комиссия 0 молча не принимается).
//! Кто вызывает: trade.rs, resolve.rs.
//! От чего зависит: num.rs, ctx.rs.
//! Настройки: нет.
//!
//! Официально — «Order Book Trading → Trade → GET / Order details» https://www.okx.com/docs-v5/en/: accFillSz
//! («Running total of filled quantity since order creation»; у спота-лимита — базовая монета), avgPx («Average filled price. If none is filled, it
//! will return ""»), fee («Sign convention: negative = net fee paid to platform; positive = net rebate received from
//! platform»), feeCcy, sz.

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Fee, TradeRequest, TradeResult, TradeStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::num;

/// Факт ордера из ответа OKX (без сети, для тестов).
#[derive(Debug, Clone, PartialEq)]
pub struct Fact {
    /// sz — отправленное количество.
    pub size: Decimal,
    /// accFillSz — исполнено.
    pub filled: Decimal,
    /// avgPx — средняя цена; 0 — ничего не исполнено.
    pub avg: Decimal,
    /// Списано комиссии (−fee; отрицательное — rebate).
    pub fee: Decimal,
    /// feeCcy; пусто — ничего не исполнено.
    pub fee_asset: String,
}

impl Fact {
    /// Статус ядра: ничего — NotFilled, меньше sz — PartiallyFilled, иначе Filled. Ошибок нет.
    pub fn status(&self) -> TradeStatus {
        if self.filled.is_zero() {
            TradeStatus::NotFilled
        } else if self.filled < self.size {
            TradeStatus::PartiallyFilled
        } else {
            TradeStatus::Filled
        }
    }
}

/// Что делает: факт из окончательного состояния ордера. Принимает: строку data ответа GET /api/v5/trade/order.
/// Возвращает: Fact. Ошибки: текст — нет sz, поле не число; исполнено, а avgPx, fee или feeCcy нет (факт не
/// подтверждён).
pub fn parse(s: &Value) -> Result<Fact, String> {
    let m = |e: GatewayError| e.message;
    let size = num::dec(s, "sz").map_err(m)?;
    let filled = num::dec_opt(s, "accFillSz")
        .map_err(m)?
        .unwrap_or(Decimal::ZERO);
    let avg = num::dec_opt(s, "avgPx").map_err(m)?;
    let fee = num::dec_opt(s, "fee").map_err(m)?;
    let fee_asset = num::text_opt(s, "feeCcy").unwrap_or_default();
    if filled.is_zero() {
        return Ok(Fact {
            size,
            filled,
            avg: Decimal::ZERO,
            fee: Decimal::ZERO,
            fee_asset: String::new(),
        });
    }
    match (avg.filter(|a| !a.is_zero()), fee) {
        (Some(avg), Some(fee)) if !fee_asset.is_empty() => Ok(Fact {
            size,
            filled,
            avg,
            fee: -fee,
            fee_asset,
        }),
        _ => Err("исполнено, а avgPx, fee или feeCcy в ответе нет".into()),
    }
}

/// Что делает: результат ядра по окончательному состоянию. Принимает: окружение, запрос ядра, состояние, ordId.
/// Ошибки: OutcomeUnknown — разобрать не удалось или факт не подтверждён (parse).
pub fn result(
    ctx: &Ctx,
    req: TradeRequest,
    s: &Value,
    order_id: String,
) -> GatewayResult<TradeResult> {
    let f = parse(s).map_err(|why| {
        ctx.tag(GatewayError::new(
            ErrorCode::OutcomeUnknown,
            None,
            format!("Ордер OKX {order_id}: окончательное состояние не разобрано: {why}"),
        ))
        .with_raw(s.to_string())
    })?;
    Ok(TradeResult {
        request: req,
        status: f.status(),
        filled_qty: f.filled,
        avg_price: f.avg,
        fees: Fee::single(f.fee, f.fee_asset),
        venue_ref: order_id,
        chain: None,
        finished_at: Utc::now(),
        executor: None,
    })
}
