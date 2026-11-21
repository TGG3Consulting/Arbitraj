//! Файл: venues/poloniex/fills.rs
//! Что делает: факт торговой комиссии исполненного ордера Poloniex из его сделок — сколько и в какой монете (в ордере
//! комиссии нет). Сделки ждутся, пока их количество не сойдётся с filledQuantity ордера (не дольше
//! venue.order_final_wait_ms).
//! Кто вызывает: order_state.rs (result), tests/poloniex_real_data.rs (sum).
//! От чего зависит: rest.rs, num.rs, order_state.rs (сроки), ctx.rs.
//! Настройки: через order_state.rs — venue.order_poll_ms, venue.order_final_wait_ms [poloniex].
//!
//! Официально — Trades by Order Id GET /orders/{id}/trades https://api-docs.poloniex.com/spot/api/private/trade
//! («clientOrderId is not accepted»): массив сделок {id, orderId, side, matchRole, price, quantity («base units»),
//! amount, feeCurrency, feeAmount, …}. В какой монете берётся комиссия покупки — не описано: монета — feeCurrency
//! каждой сделки как есть (скидка TRX может давать комиссию в TRX — урок 23). Примеры документации: Trades by Order Id —
//! side BUY, LINK_USDT, feeCurrency LINK (комиссия покупки в получаемой монете); Trade History — side SELL,
//! feeCurrency USDT. Решение 232: части в разных монетах — список (core/types/trade.rs Fee), не ошибка.

use std::time::Instant;

use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::Fee;

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, order_state, rest};

/// Что делает: сумма сделок — (количество, комиссия по монетам).
/// Принимает: ответ /orders/{id}/trades. Возвращает: (Σ quantity, список частей комиссии по feeCurrency). Решение 232 (образец 219): сделки ордера в разных
/// монетах комиссии (например, часть — TRX со скидкой) — несколько частей, а не ошибка.
/// Ошибки: текст причины — ответ не массив, поле не число, feeAmount без feeCurrency.
pub fn sum(data: &Value) -> Result<(Decimal, Vec<Fee>), String> {
    let list = data
        .as_array()
        .ok_or_else(|| format!("сделки ордера — не массив: {data}"))?;
    let (mut qty, mut fees) = (Decimal::ZERO, Vec::new());
    for t in list {
        let get = |f: &str| num::dec_opt(t, f).map_err(|e| e.message);
        qty += get("quantity")?.unwrap_or_default();
        let amount = get("feeAmount")?.unwrap_or_default().abs();
        if amount.is_zero() {
            continue;
        }
        let c = num::text_opt(t, "feeCurrency")
            .map(|c| c.to_ascii_uppercase())
            .ok_or_else(|| format!("сделка с feeAmount без feeCurrency: {t}"))?;
        Fee::add(&mut fees, &c, amount);
    }
    Ok((qty, fees))
}

/// Что делает: комиссия ордера по его сделкам — ждёт, пока количество сделок не сойдётся с filled.
/// Принимает: id ордера, исполненное количество. Возвращает: части комиссии по монетам.
/// Ошибки: текст причины — сроки не заданы, сделки не сошлись за venue.order_final_wait_ms, разбор.
pub async fn fee(ctx: &Ctx, order_id: &str, filled: Decimal) -> Result<Vec<Fee>, String> {
    let (poll, wait) = order_state::times(ctx).map_err(|e| e.message)?;
    let path = format!("/orders/{order_id}/trades");
    let started = Instant::now();
    loop {
        let why = match rest::signed(ctx, Method::GET, &path, vec![], None, Op::Other).await {
            Ok(d) => {
                let (qty, fees) = sum(&d)?;
                if qty >= filled {
                    return Ok(fees);
                }
                format!("сделки ордера на {qty} из {filled}")
            }
            Err(e) => format!("сделки ордера: {}", e.message),
        };
        if started.elapsed() >= wait {
            return Err(format!("комиссия не подтверждена: {why}"));
        }
        tokio::time::sleep(poll).await;
    }
}
