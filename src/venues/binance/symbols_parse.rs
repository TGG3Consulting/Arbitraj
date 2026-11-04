//! Файл: venues/binance/symbols_parse.rs
//! Что делает: разбор ответа exchangeInfo Binance в правила пар (SymbolRules) и разбор прав аккаунта (permissions).
//! Кто вызывает: symbols.rs (refresh), tests/binance_real_data.rs.
//! От чего зависит: num.rs, errors.rs, symbols.rs (SymbolRules, коды статуса).
//! Настройки: нет.
//!
//! Официально:
//! - GET /api/v3/exchangeInfo (rest-api «General endpoints») — symbols[]: symbol, status, baseAsset, quoteAsset,
//!   orderTypes, isSpotTradingAllowed, permissionSets («An order can be placed if the account has at least one of the
//!   permissions in each set» — пересказ правила; проверить на живом ответе), filters.
//! - Фильтры https://developers.binance.com/legacy-docs/binance-spot-api-docs/filters — PRICE_FILTER (minPrice,
//!   maxPrice, tickSize; «0» — правило выключено), LOT_SIZE (minQty, maxQty, stepSize), NOTIONAL (minNotional,
//!   maxNotional), MIN_NOTIONAL (minNotional). PERCENT_PRICE / PERCENT_PRICE_BY_SIDE не разбираются: им нужна средняя
//!   или референсная цена — их ловит пробный ордер (order_rules.rs).
//! - GET /api/v3/account — permissions (права аккаунта, например ["SPOT"]).

use std::collections::{HashMap, HashSet};

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::GatewayResult;

use super::num;
use super::symbols::{SymbolRules, status_code};

/// Права аккаунта из ответа GET /api/v3/account (поле permissions — массив строк). Ошибка BadResponse — нет массива.
pub fn account_permissions(account: &Value) -> GatewayResult<HashSet<String>> {
    Ok(num::arr(account, "permissions")?
        .iter()
        .filter_map(|p| p.as_str().map(str::to_string))
        .collect())
}

/// Разбор exchangeInfo: пары с котируемой монетой quote (ключ — тикер монеты). perms — права аккаунта.
/// Ошибка BadResponse — нет symbols или у пары нет обязательных полей.
pub fn parse_list(
    data: &Value,
    quote: &str,
    perms: &HashSet<String>,
) -> GatewayResult<HashMap<String, SymbolRules>> {
    let mut map = HashMap::new();
    for s in num::arr(data, "symbols")? {
        if num::text_opt(s, "quoteAsset").as_deref() != Some(quote) {
            continue;
        }
        let ticker = num::text(s, "baseAsset")?;
        map.insert(ticker, parse(s, perms)?);
    }
    Ok(map)
}

/// Разбор одной пары. Ошибка BadResponse — нет symbol/status или фильтр не число.
fn parse(s: &Value, perms: &HashSet<String>) -> GatewayResult<SymbolRules> {
    let symbol = num::text(s, "symbol")?;
    let status_text = num::text(s, "status")?;
    let spot_ok = matches!(s.get("isSpotTradingAllowed"), Some(Value::Bool(true)));
    let perm_ok = permission_ok(s, perms);
    let f = |kind: &str| filter(s, kind);
    let price = f("PRICE_FILTER");
    let lot = f("LOT_SIZE");
    let notional = f("NOTIONAL");
    let min_notional = f("MIN_NOTIONAL");
    let field = |v: Option<&Value>, name: &str| -> GatewayResult<Option<Decimal>> {
        match v {
            // «0» в фильтре — правило выключено (filters.md): не ограничение.
            Some(v) => Ok(num::dec_opt(v, name)?.filter(|d| !d.is_zero())),
            None => Ok(None),
        }
    };
    Ok(SymbolRules {
        tick_size: field(price, "tickSize")?.unwrap_or(Decimal::ZERO),
        min_price: field(price, "minPrice")?,
        max_price: field(price, "maxPrice")?,
        step_size: field(lot, "stepSize")?.unwrap_or(Decimal::ZERO),
        min_qty: field(lot, "minQty")?,
        max_qty: field(lot, "maxQty")?,
        min_notional: field(notional, "minNotional")?.or(field(min_notional, "minNotional")?),
        max_notional: field(notional, "maxNotional")?,
        status: status_code(&status_text),
        status_text,
        api_buy: spot_ok && perm_ok,
        api_sell: spot_ok && perm_ok,
        order_types: s.get("orderTypes").and_then(Value::as_array).map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        }),
        symbol,
    })
}

/// Фильтр пары по filterType. None — фильтра нет. Ошибок нет.
fn filter<'a>(s: &'a Value, kind: &str) -> Option<&'a Value> {
    s.get("filters")?
        .as_array()?
        .iter()
        .find(|f| f.get("filterType").and_then(Value::as_str) == Some(kind))
}

/// Права аккаунта покрывают permissionSets пары: в каждом наборе есть хотя бы одно право аккаунта. Поля нет или это
/// не массив массивов — false (торговли через API нет, причина видна в SpotPairs). Ошибок нет.
pub fn permission_ok(s: &Value, perms: &HashSet<String>) -> bool {
    let Some(sets) = s.get("permissionSets").and_then(Value::as_array) else {
        return false;
    };
    sets.iter().all(|set| {
        set.as_array().is_some_and(|items| {
            items
                .iter()
                .any(|p| p.as_str().is_some_and(|p| perms.contains(p)))
        })
    })
}
