//! Файл: venues/htx/order_fee.rs
//! Что делает: факт торговой комиссии ордера HTX по его сделкам (matchresults): сколько и в какой монете — в монете
//! сделки (fee-currency) или списано скидкой HT / баллами HB (fee-deduct-currency, урок 23).
//! Кто вызывает: order_state.rs (result), tests/htx_real_data.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: нет.
//!
//! Официально — Get the Match Result of an Order GET /v1/order/orders/{order-id}/matchresults («50 times/2s»)
//! https://huobiapi.github.io/docs/spot/v1/en/#get-the-match-result-of-an-order: filled-fees «positive for a fee;
//! negative for a maker rebate» (бот ставит только IOC/FOK — тейкер), fee-currency — монета комиссии,
//! fee-deduct-currency — пусто: комиссия в исходной монете, «ht» или «hbpoint» — списано ими; filled-points — сколько
//! списано HT / баллами; FAQ: filled-fees и filled-points не бывают заполнены оба; fee-deduct-state «ongoing» / «done».
//! fee-currency: «transaction fee of buy order is based on base currency, transaction fee of sell order is based on
//! quote currency» (покупка — в монете, продажа — в котируемой; модель котировки book.rs та же).
//! Решение 232 (образец 219): сделки ордера в разных монетах (часть — HT, часть — монета сделки) — несколько частей
//! комиссии списком, а не ошибка.

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::Fee;

use super::ctx::{Ctx, ticker};
use super::{num, rest};

/// Что делает: комиссия ордера по его сделкам (запрос matchresults), части по монетам.
/// Принимает: окружение, order-id, получаемую монету сделки (если у сделки нет fee-currency).
/// Возвращает: список частей комиссии (core/types/trade.rs Fee). Ошибка — текст причины (запрос не прошёл, ответ не тот).
pub async fn fact(ctx: &Ctx, order_id: &str, received: &str) -> Result<Vec<Fee>, String> {
    let path = format!("/v1/order/orders/{order_id}/matchresults");
    let data = rest::get(ctx, &path, vec![])
        .await
        .map_err(|e| format!("сделки ордера (matchresults): {}", e.message))?;
    parse(&data, received)
}

/// Что делает: разбор data matchresults в части комиссии по монетам.
/// Принимает: data (массив сделок), получаемую монету (если у сделки нет fee-currency). Возвращает: список частей
/// (сделок нет — пустой). Ошибки: текст — не массив, поле не число.
pub fn parse(data: &Value, received: &str) -> Result<Vec<Fee>, String> {
    let list = match data {
        Value::Null => return Ok(Vec::new()),
        Value::Array(a) => a,
        other => return Err(format!("matchresults: data не массив: {other}")),
    };
    let mut fees = Vec::new();
    for m in list {
        let dec = |f: &str| {
            num::dec_opt(m, f)
                .map(|v| v.unwrap_or(Decimal::ZERO))
                .map_err(|_| format!("matchresults: {f} не число: {m}"))
        };
        let (coin, amount) = match num::text_opt(m, "fee-deduct-currency") {
            Some(c) => (ticker(&c), dec("filled-points")?),
            None => {
                let c = num::text_opt(m, "fee-currency").unwrap_or_else(|| received.to_string());
                (ticker(&c), dec("filled-fees")?)
            }
        };
        Fee::add(&mut fees, &coin, amount.abs());
    }
    Ok(fees)
}
