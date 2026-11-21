//! Файл: venues/bybit/fills.rs
//! Что делает: комиссия исполненного ордера Bybit и в какой монете (урок 7, 11): (1) cumFeeDetail окончательной записи
//! ордера («Cumulative trading fee details instead of cumExecFee» у спота) — если там ровно одна монета; (2) иначе —
//! исполнения ордера (поток execution.spot или GET /v5/execution/list) со сверкой суммы execQty с cumExecQty ордера
//! (как решение 118, А-🔴1 у MEXC); не сошлось за venue.order_final_wait_ms — OutcomeUnknown, а не заниженная комиссия.
//! Кто вызывает: trade.rs (commission), order_rules.rs (limit — проверка до отправки ордера).
//! От чего зависит: rest.rs, num.rs, ctx.rs, private_cache.rs, errors.rs.
//! Настройки: venue.my_trades_limit [bybit] (1..100), venue.order_poll_ms, venue.order_final_wait_ms.
//!
//! Официальная документация:
//! - «Get Open & Closed Orders» https://bybit-exchange.github.io/docs/v5/order/open-order — cumFeeDetail (json,
//!   пример {"MNT": "0.00242968"}).
//! - «Get Trade History» GET /v5/execution/list https://bybit-exchange.github.io/docs/v5/order/execution — category,
//!   orderId, limit («[1, 100]. Default: 50»); list[]: orderId, execId, execQty, execFee, feeCurrency («Trading fee
//!   currency»); «You may have multiple executions in a single order».
//! - Монета комиссии спота — enum «Spot Fee Currency Instruction» (покупка — базовая, продажа — котируемая при
//!   положительной ставке) — берётся по факту из ответа, не выводится.

use std::collections::HashMap;
use std::time::Instant;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Наибольший limit у /v5/execution/list — из официальной документации («[1, 100]»).
pub const MAX_EXEC_LIMIT: u64 = 100;

/// Читает и проверяет venue.my_trades_limit [bybit]. Ошибки: NotConfigured — не задана или вне 1..=100.
pub fn limit(ctx: &Ctx) -> GatewayResult<u64> {
    let n = ctx.int("venue.my_trades_limit")?;
    if n == 0 || n > MAX_EXEC_LIMIT {
        return Err(ctx.tag(GatewayError::not_set(
            None,
            "venue.my_trades_limit",
            format!("venue.my_trades_limit = {n}: у Bybit /v5/execution/list limit от 1 до {MAX_EXEC_LIMIT}"),
        )));
    }
    Ok(n)
}

/// Что делает: комиссия ордера. Принимает: окружение, orderId, исполнено (cumExecQty), окончательную запись ордера.
/// Возвращает: (комиссия, монета). Ошибки: BadResponse — комиссия в разных монетах; OutcomeUnknown — не подтверждена.
pub async fn commission(
    ctx: &Ctx,
    order_id: &str,
    executed: Decimal,
    order: &Value,
) -> GatewayResult<(Decimal, String)> {
    if let Some(detail) = order.get("cumFeeDetail").and_then(Value::as_object)
        && detail.len() == 1
        && let Some((coin, v)) = detail.iter().next()
        && let Some(fee) = num::to_dec(v)
    {
        return Ok((fee, coin.clone()));
    }
    let poll = std::time::Duration::from_millis(ctx.int("venue.order_poll_ms")?);
    let wait = std::time::Duration::from_millis(ctx.int("venue.order_final_wait_ms")?);
    let started = Instant::now();
    let why = loop {
        let stream = ctx.private.fills(order_id);
        let list = if stream.is_empty() {
            read(ctx, order_id).await
        } else {
            Ok(stream)
        };
        let why = match list.and_then(|l| sum(&l)) {
            Ok((_, fees)) if fees.len() > 1 => {
                let coins: Vec<&String> = fees.keys().collect();
                return Err(super::errors::bad(
                    &format!("ордер {order_id}: комиссия в разных монетах {coins:?}"),
                    "",
                ));
            }
            Ok((qty, fees)) if qty == executed => {
                let (coin, fee) = fees.into_iter().next().unwrap_or_default();
                return Ok((fee, coin));
            }
            Ok((qty, _)) => format!("сумма execQty {qty} не равна cumExecQty {executed}"),
            Err(e) => format!("исполнения не прочитаны: {}", e.message),
        };
        if started.elapsed() >= wait {
            break why;
        }
        ctx.private.wait(poll).await;
    };
    Err(ctx.tag(GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        format!("Ордер {order_id} исполнен на {executed}, но комиссия не подтверждена за venue.order_final_wait_ms: {why}"),
    )))
}

/// Сумма execQty и комиссии по монетам. Ошибки: BadResponse — поле не число.
fn sum(list: &[Value]) -> GatewayResult<(Decimal, HashMap<String, Decimal>)> {
    let mut qty = Decimal::ZERO;
    let mut fees: HashMap<String, Decimal> = HashMap::new();
    for t in list {
        qty += num::dec(t, "execQty")?;
        let fee = num::dec_opt(t, "execFee")?.unwrap_or_default();
        if let Some(c) = num::text_opt(t, "feeCurrency") {
            *fees.entry(c).or_default() += fee;
        }
    }
    Ok((qty, fees))
}

/// Исполнения ордера по REST. Ошибки: настройка limit, связь, коды Bybit, формат.
async fn read(ctx: &Ctx, order_id: &str) -> GatewayResult<Vec<Value>> {
    let p = vec![
        ("category", "spot".to_string()),
        ("orderId", order_id.to_string()),
        ("limit", limit(ctx)?.to_string()),
    ];
    let d = rest::get(ctx, "/v5/execution/list", p, Op::Other).await?;
    Ok(num::arr(&d, "list")?
        .iter()
        .filter(|t| num::text_opt(t, "orderId").as_deref() == Some(order_id))
        .cloned()
        .collect())
}
