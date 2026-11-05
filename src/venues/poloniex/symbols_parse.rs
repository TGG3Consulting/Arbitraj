//! Файл: venues/poloniex/symbols_parse.rs
//! Что делает: разбор ответа GET /markets Poloniex в правила пар (SymbolRules).
//! Кто вызывает: symbols.rs (refresh), tests/poloniex_real_data.rs.
//! От чего зависит: num.rs, symbols.rs (SymbolRules, статусы).
//! Настройки: нет (котируемую монету передаёт symbols.rs).
//!
//! Официально — Symbol Information https://api-docs.poloniex.com/spot/api/public/reference-data: массив; symbol,
//! baseCurrencyName, quoteCurrencyName, state («NORMAL, PAUSE, POST_ONLY»), tradableStartTime, symbolTradeLimit:
//! priceScale «decimal precision for price», quantityScale, amountScale, minQuantity, minAmount, maxQuantity / maxAmount
//! («0 means no check»), highestBid «maximum allowed bid», lowestAsk «minimum allowed ask». Масштаб n → шаг 10^−n
//! (документация называет поля precision; ошибки 21335 / 21336 «Invalid priceScale / quantityScale» — сверить живым
//! ордером, урок 2).

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::GatewayResult;

use super::num;
use super::symbols::{STATUS_NORMAL, STATUS_PAUSE, STATUS_POST_ONLY, STATUS_UNKNOWN, SymbolRules};

/// Наибольшая точность Decimal (28 знаков).
const MAX_SCALE: u32 = 28;

/// Разбор списка: пары с котируемой монетой quote (ключ — тикер монеты, baseCurrencyName). Пара с неразборчивым полем
/// не валит весь справочник, а попадает в skipped («символ: причина»). Ошибка BadResponse — ответ не массив.
pub fn parse_list(
    data: &Value,
    quote: &str,
) -> GatewayResult<(HashMap<String, SymbolRules>, Vec<String>)> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("markets: ответ не массив", ""))?;
    let (mut map, mut skipped) = (HashMap::new(), Vec::new());
    for s in list {
        if num::text_opt(s, "quoteCurrencyName").as_deref() != Some(quote) {
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

/// Разбор одной пары. Возвращает: правила. Ошибка BadResponse — нет обязательного поля или масштаб не целое.
pub fn parse(s: &Value) -> GatewayResult<SymbolRules> {
    let status = match num::text(s, "state")?.as_str() {
        "NORMAL" => STATUS_NORMAL,
        "PAUSE" => STATUS_PAUSE,
        "POST_ONLY" => STATUS_POST_ONLY,
        _ => STATUS_UNKNOWN,
    };
    let lim = s
        .get("symbolTradeLimit")
        .ok_or_else(|| super::errors::bad("нет symbolTradeLimit", &s.to_string()))?;
    let positive = |f: &str| num::dec_opt(lim, f).map(|v| v.filter(|x| *x > Decimal::ZERO));
    Ok(SymbolRules {
        symbol: num::text(s, "symbol")?,
        base: num::text(s, "baseCurrencyName")?,
        quote: num::text(s, "quoteCurrencyName")?,
        tick_size: step(lim, "priceScale")?,
        step_size: step(lim, "quantityScale")?,
        min_qty: positive("minQuantity")?,
        max_qty: positive("maxQuantity")?,
        min_notional: positive("minAmount")?,
        max_notional: positive("maxAmount")?,
        highest_bid: positive("highestBid")?,
        lowest_ask: positive("lowestAsk")?,
        status,
    })
}

/// Масштаб n (знаков после запятой) → шаг 10^−n. Ошибка BadResponse — не целое или больше 28.
fn step(s: &Value, field: &str) -> GatewayResult<Decimal> {
    let n = num::int(s, field)?;
    let n = u32::try_from(n)
        .ok()
        .filter(|n| *n <= MAX_SCALE)
        .ok_or_else(|| super::errors::bad(&format!("{field} = {n} вне 0…28"), &s.to_string()))?;
    Ok(Decimal::new(1, n))
}
