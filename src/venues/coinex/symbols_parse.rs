//! Файл: venues/coinex/symbols_parse.rs
//! Что делает: разбор ответа GET /v2/spot/market CoinEx в правила пар (SymbolRules).
//! Кто вызывает: symbols.rs (refresh), tests/coinex_real_data.rs.
//! От чего зависит: num.rs, symbols.rs (SymbolRules, статусы).
//! Настройки: нет (котируемую монету передаёт symbols.rs).
//!
//! Официально — Get Market Status https://docs.coinex.com/api/v2/spot/market/http/list-market: market, maker_fee_rate /
//! taker_fee_rate (ставки по умолчанию; ставка ключа — fees.rs), min_amount «Minimum transaction volume», base_ccy,
//! quote_ccy, base_ccy_precision / quote_ccy_precision «Decimal places for the base / quote currency» (int), status,
//! delisted_at, is_api_trading_available. Шага цены и количества отдельно нет — только число знаков: шаг 10^−n (сверить
//! с живым ордером, урок 2). Минимальной суммы в котируемой монете и максимумов нет. Расхождение: в примере ответа
//! is_pre_trading_available, в таблице is_pre_market_trading_available (не используется); пример — невалидный JSON
//! (висячие запятые).

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::GatewayResult;

use super::num;
use super::symbols::{
    STATUS_BIDDING, STATUS_COUNTING_DOWN, STATUS_ONLINE, STATUS_UNKNOWN, SymbolRules,
};

/// Наибольшая точность Decimal (28 знаков).
const MAX_SCALE: u32 = 28;

/// Разбор списка: пары с котируемой монетой quote (ключ — тикер монеты, base_ccy). Пара с неразборчивым полем
/// не валит весь справочник, а попадает в skipped («рынок: причина»). Ошибка BadResponse — data не массив.
pub fn parse_list(
    data: &Value,
    quote: &str,
) -> GatewayResult<(HashMap<String, SymbolRules>, Vec<String>)> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("spot/market: data не массив", ""))?;
    let (mut map, mut skipped) = (HashMap::new(), Vec::new());
    for s in list {
        if num::text_opt(s, "quote_ccy").as_deref() != Some(quote) {
            continue;
        }
        match parse(s) {
            Ok(r) => {
                map.insert(r.base.clone(), r);
            }
            Err(e) => skipped.push(format!(
                "{}: {}",
                num::text_opt(s, "market").unwrap_or_default(),
                e.message
            )),
        }
    }
    Ok((map, skipped))
}

/// Разбор одной пары. Возвращает: правила. Ошибка BadResponse — нет обязательного поля или точность не целое.
pub fn parse(s: &Value) -> GatewayResult<SymbolRules> {
    let status = match num::text(s, "status")?.as_str() {
        "online" => STATUS_ONLINE,
        "counting_down" => STATUS_COUNTING_DOWN,
        "bidding" => STATUS_BIDDING,
        _ => STATUS_UNKNOWN,
    };
    Ok(SymbolRules {
        symbol: num::text(s, "market")?,
        base: num::text(s, "base_ccy")?,
        quote: num::text(s, "quote_ccy")?,
        tick_size: step(s, "quote_ccy_precision")?,
        step_size: step(s, "base_ccy_precision")?,
        min_qty: num::dec_opt(s, "min_amount")?.filter(|m| !m.is_zero()),
        status,
        api: num::flag(s, "is_api_trading_available")?,
    })
}

/// Точность n (знаков после запятой) → шаг 10^−n. Ошибка BadResponse — не целое или больше 28.
fn step(s: &Value, field: &str) -> GatewayResult<Decimal> {
    let n = num::int(s, field)?;
    let n = u32::try_from(n)
        .ok()
        .filter(|n| *n <= MAX_SCALE)
        .ok_or_else(|| super::errors::bad(&format!("{field} = {n} вне 0…28"), &s.to_string()))?;
    Ok(Decimal::new(1, n))
}
