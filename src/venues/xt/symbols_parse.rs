//! Файл: venues/xt/symbols_parse.rs
//! Что делает: разбор ответа /v4/public/symbol XT.com в правила пар (SymbolRules): состояние, торговля через API,
//! фильтры PRICE / QUANTITY / QUOTE_QTY, точности, ставка тейкера пары, типы ордера и сроки действия.
//! Кто вызывает: symbols.rs (refresh, в spawn_blocking), tests/xt_real_data.rs.
//! От чего зависит: num.rs, errors.rs, ctx.rs (тикер), symbols.rs (SymbolRules, коды состояния).
//! Настройки: нет.
//!
//! Официально (https://doc.xt.com/#marketsymbol, `_market/2symbol.md`): result {time, version, symbols[]}; пара:
//! symbol («btc_usdt»), state (ONLINE, OFFLINE, DELISTED), tradingEnabled, openapiEnabled («Openapi transaction is
//! available or not»), baseCurrency, quoteCurrency, pricePrecision, quantityPrecision, takerFeeRate, makerFeeRate,
//! orderTypes, timeInForces, filters[{filter: PRICE|QUANTITY (min, max, tickSize) | QUOTE_QTY (min) |
//! PROTECTION_LIMIT | PROTECTION_MARKET | PROTECTION_ONLINE}]. «Each of the above items can be null, when it is null,
//! it means that this item is no longer restricted». Шаг — от минимума: «price must be equal to minPrice+(integer
//! multiple of tickSize)», то же у количества. PROTECTION_LIMIT считается от последней цены сделки — её у адаптера нет
//! (лента сделок не слушается): не проверяется, ловится отказом ORDER_F05xx (отчёт решения 167).
//! Решение 190: верхний предел (max) больше, чем помещается в Decimal (~7.9·10^28), — «без ограничения», как null по
//! документации. Живой ответ 06.11.2026: pepe_usdt QUANTITY max = "1000000000000000000000000000000" (10^30) ронял
//! разбор ВСЕГО справочника — подписка XT не поднималась (tests/data/xt_live_2026-11-06).

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::GatewayResult;

use super::ctx::ticker;
use super::num;
use super::symbols::{Range, SymbolRules, state_code};

/// Разобранный справочник: версия ответа и пары с котируемой монетой (ключ — тикер ядра).
pub struct Parsed {
    /// version ответа (при той же версии XT список не присылает).
    pub version: Option<String>,
    /// None — список не прислан (версия не изменилась).
    pub map: Option<HashMap<String, SymbolRules>>,
}

/// Что делает: разбор result /v4/public/symbol. Принимает: result, котируемую монету (venue.quote_asset, регистр не
/// важен). Возвращает: Parsed. Ошибка BadResponse — у пары нет обязательных полей или фильтр не число.
pub fn parse_list(data: &Value, quote: &str) -> GatewayResult<Parsed> {
    let version = num::text_opt(data, "version");
    let Some(list) = data.get("symbols").and_then(Value::as_array) else {
        return Ok(Parsed { version, map: None });
    };
    let mut map = HashMap::new();
    for s in list {
        let q = num::text_opt(s, "quoteCurrency").unwrap_or_default();
        if !q.eq_ignore_ascii_case(quote) {
            continue;
        }
        let r = parse(s)?;
        map.insert(r.ticker.clone(), r);
    }
    Ok(Parsed {
        version,
        map: Some(map),
    })
}

/// Разбор одной пары. Ошибка BadResponse — нет symbol/state/baseCurrency или поле не число.
fn parse(s: &Value) -> GatewayResult<SymbolRules> {
    let symbol = num::text(s, "symbol")?;
    let state_text = num::text(s, "state")?;
    let flag = |f: &str| matches!(s.get(f), Some(Value::Bool(true)));
    let api = flag("tradingEnabled") && flag("openapiEnabled");
    let precision = |f: &str| num::text_opt(s, f).and_then(|p| p.parse::<u32>().ok());
    let list = |f: &str| {
        s.get(f).and_then(Value::as_array).map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
    };
    let quote_min = match filter(s, "QUOTE_QTY") {
        Some(f) => num::dec_opt(f, "min")?,
        None => None,
    };
    Ok(SymbolRules {
        ticker: ticker(&num::text(s, "baseCurrency")?),
        price: range(filter(s, "PRICE"), precision("pricePrecision"))?,
        qty: range(filter(s, "QUANTITY"), precision("quantityPrecision"))?,
        quote_min,
        taker_fee: num::dec_opt(s, "takerFeeRate")?,
        status: state_code(&state_text),
        state_text,
        trading_enabled: flag("tradingEnabled"),
        openapi_enabled: flag("openapiEnabled"),
        api_buy: api,
        api_sell: api,
        order_types: list("orderTypes"),
        time_in_forces: list("timeInForces"),
        symbol,
    })
}

/// Фильтр PRICE или QUANTITY в пределы и шаг; null — без ограничения. Ошибка BadResponse — поле не число.
fn range(f: Option<&Value>, precision: Option<u32>) -> GatewayResult<Range> {
    let get = |n: &str| match f {
        Some(f) => num::dec_opt(f, n),
        None => Ok(None),
    };
    Ok(Range {
        min: get("min")?,
        max: match f {
            Some(f) => max_or_unbounded(f)?,
            None => None,
        },
        tick: get("tickSize")?.filter(|t| *t > Decimal::ZERO),
        precision,
    })
}

/// Верхний предел фильтра (решение 190): целое больше предела Decimal — без ограничения (None), как null по
/// документации. Ошибка BadResponse — поле не число (в т.ч. дробь или знак, не помещающиеся в Decimal).
fn max_or_unbounded(f: &Value) -> GatewayResult<Option<rust_decimal::Decimal>> {
    match num::dec_opt(f, "max") {
        Ok(v) => Ok(v),
        Err(e) => {
            let raw = num::text_opt(f, "max").unwrap_or_default();
            let digits = raw.trim();
            if !digits.is_empty() && digits.len() > 28 && digits.bytes().all(|b| b.is_ascii_digit())
            {
                Ok(None)
            } else {
                Err(e)
            }
        }
    }
}

/// Фильтр пары по полю filter. None — фильтра нет. Ошибок нет.
fn filter<'a>(s: &'a Value, kind: &str) -> Option<&'a Value> {
    s.get("filters")?
        .as_array()?
        .iter()
        .find(|f| f.get("filter").and_then(Value::as_str) == Some(kind))
}
