//! Файл: venues/binance/fills.rs
//! Что делает: комиссия исполненного ордера Binance и её монета. Сначала — fills[] ответа FULL (price, qty, commission,
//! commissionAsset): если сумма qty равна executedQty — это весь факт. Иначе (ответ не FULL, ордер найден по своему ID
//! после «итог неизвестен», сделок больше, чем в ответе) — сделки ордера GET /api/v3/myTrades с limit и сверка суммы
//! qty с executedQty; не сошлось за venue.order_final_wait_ms — OutcomeUnknown с фактом, а не заниженная комиссия.
//! Кто вызывает: order_state.rs (result), order_rules.rs (limit — проверка до отправки ордера).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs.
//! Настройки: venue.my_trades_limit [binance] (обязательна), venue.order_poll_ms, venue.order_final_wait_ms.
//!
//! Официально (rest-api «Account endpoints»): GET /api/v3/myTrades — symbol (обязателен), orderId («can only be used
//! in combination with symbol»), limit («Default 500; max 1000»); ответ — массив: orderId, qty, commission,
//! commissionAsset. Скидка BNB (faqs/commission_faq): комиссия в BNB, если скидка включена и BNB хватает, иначе — из
//! полученной монеты; разные монеты комиссии у одного ордера — факт, показывается как ошибка разбора (ручная сверка).

use std::time::{Duration, Instant};

use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Наибольший limit у myTrades — из официальной документации («max 1000»).
pub const MAX_MY_TRADES_LIMIT: u64 = 1000;

/// Что делает: читает и проверяет venue.my_trades_limit. Возвращает: 1..=1000. Ошибка NotConfigured.
pub fn limit(ctx: &Ctx) -> GatewayResult<u64> {
    let n = ctx.int("venue.my_trades_limit")?;
    if n == 0 || n > MAX_MY_TRADES_LIMIT {
        return Err(ctx.tag(GatewayError::not_set(
            None,
            "venue.my_trades_limit",
            format!("venue.my_trades_limit = {n}: у Binance от 1 до {MAX_MY_TRADES_LIMIT} (myTrades, «max 1000»)"),
        )));
    }
    Ok(n)
}

/// Сумма сделок: qty, commission, монета, сколько сделок, разные монеты («первая, другая»).
#[derive(Debug, Default)]
pub struct Sum {
    pub qty: Decimal,
    pub fee: Decimal,
    pub asset: String,
    pub count: usize,
    pub mixed: Option<String>,
}

/// Что делает: сумма по сделкам (fills ответа или myTrades). Принимает: сделки. Возвращает: Sum.
/// Ошибка BadResponse — qty или commission не число.
pub fn sum<'a>(trades: impl Iterator<Item = &'a Value>) -> GatewayResult<Sum> {
    let mut r = Sum::default();
    for t in trades {
        let a = num::text_opt(t, "commissionAsset").unwrap_or_default();
        if !r.asset.is_empty() && !a.is_empty() && a != r.asset {
            r.mixed = Some(format!("{}, {a}", r.asset));
        }
        if r.asset.is_empty() {
            r.asset = a;
        }
        r.qty += num::dec(t, "qty")?;
        r.fee += num::dec_opt(t, "commission")?.unwrap_or_default().abs();
        r.count += 1;
    }
    Ok(r)
}

/// Что делает: комиссия из fills ответа FULL. Принимает: ответ ордера, executedQty.
/// Возвращает: Some((комиссия, монета)) — fills покрывают всё исполненное; None — fills нет или не сошлись.
/// Ошибка BadResponse — комиссия в разных монетах или поле не число.
pub fn from_fills(order: &Value, executed: Decimal) -> GatewayResult<Option<(Decimal, String)>> {
    let Some(list) = order.get("fills").and_then(Value::as_array) else {
        return Ok(None);
    };
    let s = sum(list.iter())?;
    mixed(&s, &num::text_opt(order, "orderId").unwrap_or_default())?;
    Ok((s.count > 0 && s.qty == executed).then_some((s.fee, s.asset)))
}

/// Разные монеты комиссии — ошибка с фактом. Ошибок, кроме этой, нет.
fn mixed(s: &Sum, order_id: &str) -> GatewayResult<()> {
    match &s.mixed {
        Some(m) => Err(super::errors::bad(
            &format!(
                "ордер {order_id}: комиссия в разных монетах ({m}), всего {} — сверить вручную",
                s.fee
            ),
            "",
        )),
        None => Ok(()),
    }
}

/// Что делает: комиссия ордера по myTrades после сверки суммы qty с executedQty (повторы до venue.order_final_wait_ms).
/// Принимает: окружение, символ, orderId, executedQty. Возвращает: (комиссия, монета).
/// Ошибки: OutcomeUnknown — не сошлось или не читается; BadResponse — разные монеты.
pub async fn commission(
    ctx: &Ctx,
    symbol: &str,
    order_id: &str,
    executed: Decimal,
) -> GatewayResult<(Decimal, String)> {
    let poll = Duration::from_millis(ctx.int("venue.order_poll_ms")?);
    let wait = Duration::from_millis(ctx.int("venue.order_final_wait_ms")?);
    let started = Instant::now();
    let why = loop {
        let why = match read(ctx, symbol, order_id).await {
            Ok(s) if s.mixed.is_some() => return mixed(&s, order_id).map(|_| (s.fee, s.asset)),
            Ok(s) if s.qty == executed => return Ok((s.fee, s.asset)),
            Ok(s) => format!(
                "сумма qty сделок {} (сделок {}) не равна executedQty {executed}, комиссия по ним {} {}",
                s.qty, s.count, s.fee, s.asset
            ),
            Err(e) => format!("сделки не прочитаны: {}", e.message),
        };
        if started.elapsed() >= wait {
            break why;
        }
        tokio::time::sleep(poll).await;
    };
    Err(ctx.tag(GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        format!(
            "Ордер {order_id} ({symbol}) исполнен на {executed}, но комиссия не подтверждена за venue.order_final_wait_ms: {why}"
        ),
    )))
}

/// Одно чтение myTrades по ордеру. Ошибки: настройка limit, связь, коды Binance, ответ не массив.
async fn read(ctx: &Ctx, symbol: &str, order_id: &str) -> GatewayResult<Sum> {
    let p = vec![
        ("symbol", symbol.to_string()),
        ("orderId", order_id.to_string()),
        ("limit", limit(ctx)?.to_string()),
    ];
    let data = rest::signed(ctx, Method::GET, "/api/v3/myTrades", p, Op::Other).await?;
    let trades = data
        .as_array()
        .ok_or_else(|| super::errors::bad("myTrades не массив", &data.to_string()))?;
    sum(trades
        .iter()
        .filter(|t| num::text_opt(t, "orderId").as_deref() == Some(order_id)))
}
