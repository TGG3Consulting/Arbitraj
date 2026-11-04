//! Файл: venues/bitget/symbols_parse.rs
//! Что делает: разбор ответа GET /api/v2/spot/public/symbols Bitget в правила пар (SymbolRules).
//! Кто вызывает: symbols.rs (refresh), tests/bitget_real_data.rs.
//! От чего зависит: num.rs, symbols.rs (SymbolRules, статусы).
//! Настройки: нет (котируемую монету передаёт symbols.rs).
//!
//! Официально — Get Symbol Info https://www.bitget.com/docs/catalog/classic-spot-market/classic-spot-market#get-symbol-info:
//! symbol, baseCoin, quoteCoin, pricePrecision «Price precision», quantityPrecision «Amount precision», quotePrecision,
//! status («offline», «gray», «online», «halt»), minTradeUSDT «Minimum trading volume (USDT)», buyLimitPriceRatio /
//! sellLimitPriceRatio, takerFeeRate / makerFeeRate (ставки по умолчанию — ставка ключа берётся в fees.rs),
//! minTradeAmount / maxTradeAmount — «obsolete» (не используются). maxLimitOrderValue есть только в примере ответа, в
//! таблице полей нет — не используется (проверить на живом ответе). Точность → шаг 10^−n (документация называет поля
//! precision, шага нет — сверить с живым ордером, урок 2).

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::GatewayResult;

use super::num;
use super::symbols::{
    STATUS_GRAY, STATUS_HALT, STATUS_OFFLINE, STATUS_ONLINE, STATUS_UNKNOWN, SymbolRules,
};

/// Котируемая монета, к которой относится minTradeUSDT («Minimum trading volume (USDT)»).
const USDT: &str = "USDT";
/// Наибольшая точность Decimal (28 знаков).
const MAX_SCALE: u32 = 28;

/// Разбор списка: пары с котируемой монетой quote (ключ — тикер монеты, baseCoin). Пара с неразборчивым полем
/// не валит весь справочник, а попадает в skipped («символ: причина»). Ошибка BadResponse — data не массив.
pub fn parse_list(
    data: &Value,
    quote: &str,
) -> GatewayResult<(HashMap<String, SymbolRules>, Vec<String>)> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("symbols: data не массив", ""))?;
    let (mut map, mut skipped) = (HashMap::new(), Vec::new());
    for s in list {
        if num::text_opt(s, "quoteCoin").as_deref() != Some(quote) {
            continue;
        }
        match parse(s) {
            Ok(r) => {
                map.insert(r.base.clone(), r);
            }
            Err(e) => skipped.push(format!(
                "{}: {}",
                num::text_opt(s, "symbol").unwrap_or_default(),
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
        "offline" => STATUS_OFFLINE,
        "gray" => STATUS_GRAY,
        "halt" => STATUS_HALT,
        _ => STATUS_UNKNOWN,
    };
    let quote = num::text(s, "quoteCoin")?;
    let min_notional = if quote == USDT {
        num::dec_opt(s, "minTradeUSDT")?
    } else {
        None
    };
    Ok(SymbolRules {
        symbol: num::text(s, "symbol")?,
        base: num::text(s, "baseCoin")?,
        quote,
        tick_size: step(s, "pricePrecision")?,
        step_size: step(s, "quantityPrecision")?,
        min_notional,
        status,
        buy_ratio: num::dec_opt(s, "buyLimitPriceRatio")?,
        sell_ratio: num::dec_opt(s, "sellLimitPriceRatio")?,
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
