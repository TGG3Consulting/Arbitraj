//! Файл: venues/mexc/symbols_parse.rs
//! Что делает: разбор ответов exchangeInfo и selfSymbols MEXC в правила пар (SymbolRules).
//! Кто вызывает: symbols.rs (refresh), tests/mexc_real_data.rs.
//! От чего зависит: num.rs, errors.rs, symbols.rs (SymbolRules, STATUS_ONLINE).
//! Настройки: нет.
//!
//! Официальная документация: GET /api/v3/exchangeInfo https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/exchange-information
//! (status, isSpotTradingAllowed, tradeSideType, baseSizePrecision, quoteAmountPrecision, maxQuoteAmount,
//! baseAssetPrecision, quotePrecision, quoteAssetPrecision, orderTypes); GET /api/v3/selfSymbols
//! https://www.mexc.com/api-docs/spot-v3/spot-account-trade/user-api-default-symbol. Шаги — решение 107 (symbols.rs).
//! Решение 390 (t180): фильтр PERCENT_PRICE_BY_SIDE — правило ценового коридора пары (price_band).

use std::collections::{HashMap, HashSet};

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::GatewayResult;
use crate::gateway::price_band::PriceBandRule;

use super::num;
use super::symbols::{STATUS_ONLINE, SymbolRules};

/// Список пар, разрешённых ключу (selfSymbols): массив символов в data (ответ в обёртке {code: 200, data}).
/// Ошибка BadResponse — не массив строк.
pub fn self_symbols(data: &Value) -> GatewayResult<HashSet<String>> {
    let arr = data
        .as_array()
        .ok_or_else(|| super::errors::bad("selfSymbols: data не массив", &data.to_string()))?;
    arr.iter()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| super::errors::bad("selfSymbols: не строка", &v.to_string()))
        })
        .collect()
}

/// Разбор exchangeInfo: пары с котируемой монетой quote (ключ — тикер монеты). allowed — selfSymbols ключа.
/// Ошибка BadResponse — неверный формат.
pub fn parse_list(
    data: &Value,
    quote: &str,
    allowed: &HashSet<String>,
) -> GatewayResult<HashMap<String, SymbolRules>> {
    let mut map = HashMap::new();
    for s in num::arr(data, "symbols")? {
        if num::text_opt(s, "quoteAsset").as_deref() != Some(quote) {
            continue;
        }
        let ticker = num::text(s, "baseAsset")?;
        map.insert(ticker, parse(s, allowed)?);
    }
    Ok(map)
}

/// Разбор одной пары. Ошибка BadResponse — нет обязательных полей.
fn parse(s: &Value, allowed: &HashSet<String>) -> GatewayResult<SymbolRules> {
    let symbol = num::text(s, "symbol")?;
    let status = match num::text(s, "status")?.as_str() {
        "1" | "ENABLED" => STATUS_ONLINE,
        "2" => 2,
        "3" => 3,
        _ => -1,
    };
    let spot_ok = matches!(s.get("isSpotTradingAllowed"), Some(Value::Bool(true)));
    let own = allowed.contains(&symbol);
    // tradeSideType: 1 все, 2 только покупка, 3 только продажа, 4 закрытие (нет поля — без ограничения стороны).
    let side = num::text_opt(s, "tradeSideType").unwrap_or_else(|| "1".into());
    let (buy_side, sell_side) = match side.as_str() {
        "2" => (true, false),
        "3" => (false, true),
        "4" => (false, false),
        _ => (true, true),
    };
    Ok(SymbolRules {
        tick_size: pow10_neg(num::int(s, "quotePrecision")?),
        step_size: pow10_neg(num::int(s, "baseAssetPrecision")?),
        base_precision: num::int(s, "baseAssetPrecision")?.clamp(0, 28) as u32,
        quote_asset_precision: num::int(s, "quoteAssetPrecision")?.clamp(0, 28) as u32,
        min_notional: num::dec_opt(s, "quoteAmountPrecision")?,
        max_notional: num::dec_opt(s, "maxQuoteAmount")?,
        min_qty: num::dec_opt(s, "baseSizePrecision")?,
        status,
        api_buy: spot_ok && own && buy_side,
        api_sell: spot_ok && own && sell_side,
        order_types: order_types(s),
        price_band: price_band(s)?,
        symbol,
    })
}

/// Что делает: решение 390 (t180) — ценовой коридор пары из фильтра PERCENT_PRICE_BY_SIDE exchangeInfo. Официально
/// (https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/exchange-information): «lastPrice means using the latest
/// trade price»; «For buy orders (only for LIMIT, IMMEDIATE_OR_CANCEL, FILL_OR_KILL): orderPrice <= lastPrice * (1 +
/// bidMultiplierUp)»; «For sell orders: orderPrice >= lastPrice * (1 - askMultiplierDown)». Других границ в
/// документации нет. Живая запись 2026-11-02 (tests/data/mexc_2026-11-02/exchange_info.json): CAKEUSDT —
/// bidMultiplierUp "0.2", askMultiplierDown "0.2". Принимает: пару exchangeInfo. Возвращает: правило; None — фильтра
/// нет или в нём нет ни одного множителя. Ошибка BadResponse — множитель не число.
pub fn price_band(s: &Value) -> GatewayResult<Option<PriceBandRule>> {
    let Some(f) = s.get("filters").and_then(Value::as_array).and_then(|a| {
        a.iter()
            .find(|f| f.get("filterType").and_then(Value::as_str) == Some("PERCENT_PRICE_BY_SIDE"))
    }) else {
        return Ok(None);
    };
    let rule = PriceBandRule {
        buy_up: num::dec_opt(f, "bidMultiplierUp")?.map(|m| Decimal::ONE + m),
        sell_down: num::dec_opt(f, "askMultiplierDown")?.map(|m| Decimal::ONE - m),
        reference:
            "последняя цена сделки (lastPrice; GET /api/v3/ticker/price, поле price «Last price»)"
                .into(),
        ..PriceBandRule::default()
    };
    Ok((!rule.is_empty()).then_some(rule))
}

/// 10^−n (n знаков после запятой): 2 → 0.01, 0 → 1. Отрицательное n — 1. Ошибок нет.
fn pow10_neg(n: i64) -> Decimal {
    let n = n.clamp(0, 28) as u32;
    Decimal::new(1, n)
}

/// Типы ордера пары (orderTypes — массив строк, живая запись 2026-11-02: LIMIT, MARKET, LIMIT_MAKER).
/// None — поля нет или это не массив строк. Ошибок нет.
fn order_types(s: &Value) -> Option<Vec<String>> {
    s.get("orderTypes")?
        .as_array()?
        .iter()
        .map(|v| v.as_str().map(str::to_string))
        .collect()
}
