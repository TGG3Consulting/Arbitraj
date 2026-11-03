//! Файл: venues/bybit/symbols_parse.rs
//! Что делает: разбор ответа GET /v5/market/instruments-info?category=spot в правила пар (SymbolRules).
//! Кто вызывает: symbols.rs (refresh), tests/bybit_real_data.rs.
//! От чего зависит: num.rs, errors.rs, symbols.rs (SymbolRules, status_code).
//! Настройки: нет.
//!
//! Официальная документация «Get Instruments Info» https://bybit-exchange.github.io/docs/v5/market/instrument, вкладка
//! Spot: list[] — symbol, baseCoin, quoteCoin, status (enum: PreLaunch, PendingOpen, Trading, Delivering, Closed;
//! «Spot has Trading only»), lotSizeFilter.basePrecision («The precision of base coin» — шаг количества, пример
//! "0.000001"), lotSizeFilter.minOrderAmt («Minimum order amount»), lotSizeFilter.maxLimitOrderQty («Maximum Limit order
//! quantity»), priceFilter.tickSize («The step to increase/reduce order price»). minOrderQty — «deprecated, no longer
//! check», maxOrderAmt — «deprecated, no longer check»: не применяются. «Spot does not support pagination».

use std::collections::HashMap;

use serde_json::Value;

use crate::gateway::errors::GatewayResult;

use super::num;
use super::symbols::{SymbolRules, status_code};

/// Разбор instruments-info: пары с котируемой монетой quote (ключ — тикер монеты).
/// Ошибка BadResponse — нет list или у пары нет обязательных полей.
pub fn parse_list(data: &Value, quote: &str) -> GatewayResult<HashMap<String, SymbolRules>> {
    let mut map = HashMap::new();
    for s in num::arr(data, "list")? {
        if num::text_opt(s, "quoteCoin").as_deref() != Some(quote) {
            continue;
        }
        let ticker = num::text(s, "baseCoin")?;
        map.insert(ticker, parse(s)?);
    }
    Ok(map)
}

/// Разбор одной пары. Ошибка BadResponse — нет обязательных полей.
fn parse(s: &Value) -> GatewayResult<SymbolRules> {
    let lot = s
        .get("lotSizeFilter")
        .ok_or_else(|| super::errors::bad("нет lotSizeFilter", &s.to_string()))?;
    let price = s
        .get("priceFilter")
        .ok_or_else(|| super::errors::bad("нет priceFilter", &s.to_string()))?;
    let status = num::text(s, "status")?;
    Ok(SymbolRules {
        symbol: num::text(s, "symbol")?,
        tick_size: num::dec(price, "tickSize")?,
        step_size: num::dec(lot, "basePrecision")?,
        min_notional: num::dec_opt(lot, "minOrderAmt")?,
        max_qty: num::dec_opt(lot, "maxLimitOrderQty")?,
        status: status_code(&status),
        status_text: status,
    })
}
