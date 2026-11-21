//! Файл: venues/gate/symbols_parse.rs
//! Что делает: разбор ответов GET /spot/currency_pairs и GET /account/detail Gate в правила пар (SymbolRules).
//! Кто вызывает: symbols.rs (refresh), tests/gate_real_data.rs.
//! От чего зависит: num.rs, errors.rs, symbols.rs (SymbolRules, коды статуса).
//! Настройки: нет.
//!
//! Официальная документация, API v4 → Spot → «Query all supported currency pairs» (GET /spot/currency_pairs)
//! https://www.gate.com/docs/developers/apiv4/en/ ; модель CurrencyPair (SDK
//! https://github.com/gate/gateapi-python/blob/master/docs/CurrencyPair.md): id, base, quote, min_base_amount,
//! min_quote_amount, max_base_amount, max_quote_amount («null means no limit»), amount_precision («Quantity precision»),
//! precision («Price precision»), trade_status («untradable / buyable / sellable / tradable»), buy_start, sell_start
//! (Unix, секунды). API v4 → Account → «Retrieve user account information» (GET /account/detail, модель AccountDetail):
//! currency_pairs — «Trading pair whitelist» ключа.
//!
//! Шага цены и количества в документации нет — есть целые precision / amount_precision. Как у MEXC (решение 107):
//! шаг = 10^−precision. Толкование проверяется на записанном ответе и первом ордере (вопрос Тиграну, docs/TASKS.md);
//! пробного ордера у Gate нет, отказ «INVALID_PRECISION» — ордер не исполнен, деньги не двигаются.

use std::collections::{HashMap, HashSet};

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::GatewayResult;

use super::num;
use super::symbols::{self, SymbolRules};

/// Белый список пар ключа (AccountDetail.currency_pairs). Пусто или нет поля — ограничений нет.
/// Ошибка BadResponse — поле есть, но это не массив строк.
pub fn whitelist(detail: &Value) -> GatewayResult<HashSet<String>> {
    match detail.get("currency_pairs") {
        None | Some(Value::Null) => Ok(HashSet::new()),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| super::errors::bad("currency_pairs: не строка", &v.to_string()))
            })
            .collect(),
        Some(other) => Err(super::errors::bad(
            "currency_pairs не массив",
            &other.to_string(),
        )),
    }
}

/// Разбор списка пар: пары с котируемой монетой quote (ключ — тикер монеты). allowed — белый список ключа
/// (пусто — все), now_s — сейчас, Unix секунды (buy_start / sell_start). Ошибка BadResponse — неверный формат.
pub fn parse_list(
    data: &Value,
    quote: &str,
    allowed: &HashSet<String>,
    now_s: i64,
) -> GatewayResult<HashMap<String, SymbolRules>> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("currency_pairs не массив", &data.to_string()))?;
    let mut map = HashMap::new();
    for s in list {
        if num::text_opt(s, "quote").as_deref() != Some(quote) {
            continue;
        }
        let ticker = num::text(s, "base")?;
        map.insert(ticker, parse(s, allowed, now_s)?);
    }
    Ok(map)
}

/// Разбор одной пары. Ошибка BadResponse — нет обязательных полей.
fn parse(s: &Value, allowed: &HashSet<String>, now_s: i64) -> GatewayResult<SymbolRules> {
    let symbol = num::text(s, "id")?;
    let status = symbols::status_code(num::text_opt(s, "trade_status").as_deref());
    let own = allowed.is_empty() || allowed.contains(&symbol);
    let started = |f: &str| {
        num::text_opt(s, f)
            .and_then(|t| t.parse::<i64>().ok())
            .is_none_or(|t| t <= now_s)
    };
    let (buy_side, sell_side) = match status {
        symbols::TRADABLE => (true, true),
        symbols::BUYABLE => (true, false),
        symbols::SELLABLE => (false, true),
        _ => (false, false),
    };
    let amount_precision = num::int(s, "amount_precision")?;
    let price_precision = num::int(s, "precision")?;
    Ok(SymbolRules {
        tick_size: pow10_neg(price_precision),
        step_size: pow10_neg(amount_precision),
        base_precision: amount_precision.clamp(0, 28) as u32,
        min_qty: num::dec_opt(s, "min_base_amount")?,
        max_qty: num::dec_opt(s, "max_base_amount")?,
        min_notional: num::dec_opt(s, "min_quote_amount")?,
        max_notional: num::dec_opt(s, "max_quote_amount")?,
        status,
        api_buy: own && buy_side && started("buy_start"),
        api_sell: own && sell_side && started("sell_start"),
        symbol,
    })
}

/// 10^−n (n знаков после запятой): 2 → 0.01, 0 → 1. Отрицательное n — 1. Ошибок нет.
fn pow10_neg(n: i64) -> Decimal {
    Decimal::new(1, n.clamp(0, 28) as u32)
}
