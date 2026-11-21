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
//! полученной монеты. Решение 219: разные монеты комиссии у одного ордера (часть в BNB, часть в монете) — не ошибка,
//! а сумма по каждой монете (список комиссий ядра, core/types/trade.rs Fee).

use std::time::{Duration, Instant};

use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::Fee;
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

/// Сумма сделок: qty, комиссия по монетам (решение 219), сколько сделок.
#[derive(Debug, Default)]
pub struct Sum {
    pub qty: Decimal,
    pub fees: Vec<Fee>,
    pub count: usize,
}

/// Что делает: сумма по сделкам (fills ответа или myTrades). Принимает: сделки. Возвращает: Sum.
/// Ошибка BadResponse — qty или commission не число.
pub fn sum<'a>(trades: impl Iterator<Item = &'a Value>) -> GatewayResult<Sum> {
    let mut r = Sum::default();
    for t in trades {
        let a = num::text_opt(t, "commissionAsset").unwrap_or_default();
        r.qty += num::dec(t, "qty")?;
        let fee = num::dec_opt(t, "commission")?.unwrap_or_default().abs();
        Fee::add(&mut r.fees, &a, fee);
        r.count += 1;
    }
    Ok(r)
}

/// Что делает: комиссия из fills ответа FULL. Принимает: ответ ордера, executedQty.
/// Возвращает: Some(комиссия по монетам) — fills покрывают всё исполненное; None — fills нет или не сошлись.
/// Ошибка BadResponse — поле не число.
pub fn from_fills(order: &Value, executed: Decimal) -> GatewayResult<Option<Vec<Fee>>> {
    let Some(list) = order.get("fills").and_then(Value::as_array) else {
        return Ok(None);
    };
    let s = sum(list.iter())?;
    Ok((s.count > 0 && s.qty == executed).then_some(s.fees))
}

/// Что делает: комиссия ордера по myTrades после сверки суммы qty с executedQty (повторы до venue.order_final_wait_ms).
/// Принимает: окружение, символ, orderId, executedQty. Возвращает: комиссию по монетам.
/// Ошибки: OutcomeUnknown — не сошлось или не читается.
pub async fn commission(
    ctx: &Ctx,
    symbol: &str,
    order_id: &str,
    executed: Decimal,
) -> GatewayResult<Vec<Fee>> {
    let poll = Duration::from_millis(ctx.int("venue.order_poll_ms")?);
    let wait = Duration::from_millis(ctx.int("venue.order_final_wait_ms")?);
    let started = Instant::now();
    let why = loop {
        let why = match read(ctx, symbol, order_id).await {
            Ok(s) if s.qty == executed => return Ok(s.fees),
            Ok(s) => format!(
                "сумма qty сделок {} (сделок {}) не равна executedQty {executed}, комиссия по ним {}",
                s.qty,
                s.count,
                Fee::text(&s.fees)
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
