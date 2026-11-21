//! Файл: venues/okx/symbols_parse.rs
//! Что делает: разбор ответа GET /api/v5/public/instruments?instType=SPOT OKX в правила пар (SymbolRules).
//! Кто вызывает: symbols.rs (refresh), tests/okx_real_data.rs.
//! От чего зависит: num.rs, symbols.rs (SymbolRules, статусы).
//! Настройки: нет (котируемую монету передаёт symbols.rs).
//!
//! Официально — «Public Data → REST API → Get instruments» https://www.okx.com/docs-v5/en/ (20 запросов / 2 с, IP +
//! instType): instId («Instrument ID, e.g. BTC-USDT»), baseCcy / quoteCcy («Only applicable to SPOT/MARGIN»), tickSz
//! («Minimum price increment»), lotSz («All order quantities (sz) must be a multiple of lotSz»; у спота — в базовой
//! монете), minSz («sz ≥ minSz AND sz is a multiple of lotSz»), maxLmtSz («The maximum order quantity of a single limit
//! order»; спот — базовая монета), maxLmtAmt («Max USD amount for a single limit order» — в USD, не в USDT: правилами не
//! сверяется, отказ даст биржа), state (live, suspend, preopen, test «can't be traded», rebase «can't be traded during
//! rebasing», post_only «only post-only orders are accepted»), ruleType (normal, pre_market). Минимальной суммы ордера в
//! справочнике нет — есть код 51020 «Your order should meet or exceed the minimum order amount» (отказ биржи).

use std::collections::HashMap;

use serde_json::Value;

use crate::gateway::errors::GatewayResult;

use super::num;
use super::symbols::{STATUS_OFF, STATUS_ONLINE, STATUS_OTHER, SymbolRules};

/// Разбор списка: пары с котируемой монетой quote (ключ — тикер монеты, baseCcy). Пара с неразборчивым полем не валит
/// весь справочник, а попадает в skipped («instId: причина»). Ошибка BadResponse — data не массив.
pub fn parse_list(
    data: &Value,
    quote: &str,
) -> GatewayResult<(HashMap<String, SymbolRules>, Vec<String>)> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("instruments: data не массив", ""))?;
    let (mut map, mut skipped) = (HashMap::new(), Vec::new());
    for s in list {
        if num::text_opt(s, "quoteCcy").as_deref() != Some(quote) {
            continue;
        }
        match parse(s) {
            Ok((ticker, r)) => {
                map.insert(ticker, r);
            }
            Err(e) => skipped.push(format!(
                "{}: {}",
                num::text_opt(s, "instId").unwrap_or_default(),
                e.message
            )),
        }
    }
    Ok((map, skipped))
}

/// Разбор одной пары. Возвращает: (тикер монеты, правила). Ошибка BadResponse — нет обязательного поля.
/// Торгуется ботом только state = live и ruleType = normal (пред-рынок — другие правила торговли, их бот не знает);
/// пустой ruleType — как normal (поле может не прийти у старых ответов — проверить на живом ответе).
pub fn parse(s: &Value) -> GatewayResult<(String, SymbolRules)> {
    let state = num::text(s, "state")?;
    let rule = num::text_opt(s, "ruleType").unwrap_or_else(|| "normal".into());
    let status = match (state.as_str(), rule.as_str()) {
        ("live", "normal") => STATUS_ONLINE,
        ("suspend" | "test" | "rebase", _) => STATUS_OFF,
        _ => STATUS_OTHER,
    };
    Ok((
        num::text(s, "baseCcy")?,
        SymbolRules {
            symbol: num::text(s, "instId")?,
            tick_size: num::dec(s, "tickSz")?,
            step_size: num::dec(s, "lotSz")?,
            min_qty: num::dec_opt(s, "minSz")?,
            max_qty: num::dec_opt(s, "maxLmtSz")?,
            status,
            state: format!("{state}/{rule}"),
        },
    ))
}
