//! Файл: venues/kucoin/symbols_parse.rs
//! Что делает: разбор ответа GET /api/v2/symbols KuCoin в правила пар (SymbolRules).
//! Кто вызывает: symbols.rs (refresh), tests/kucoin_real_data.rs.
//! От чего зависит: num.rs, symbols.rs (SymbolRules, статусы).
//! Настройки: нет (котируемую монету передаёт symbols.rs).
//!
//! Официально — Get All Symbols https://www.kucoin.com/docs-new/api-3470154 (спецификация SDK
//! spec/rest/api/openapi-spot-market.json): symbol («Unique code of a symbol; it will not change after renaming»),
//! baseCurrency, quoteCurrency, baseMinSize «minimum order quantity», baseMaxSize «maximum order size», baseIncrement
//! «The quantity for an order must be a positive integer multiple of this increment», priceIncrement «The price of an
//! order must be a positive integer multiple of this increment», minFunds «The minimum trading amounts»,
//! enableTrading «Available for transaction or not», callauctionIsEnabled — «The call auction status returns
//! true/false», priceLimitRate «Threshold for price protection», feeCurrency, makerFeeCoefficient /
//! takerFeeCoefficient («The actual fee needs to be multiplied by this coefficient»), st (Special Treatment).
//! quoteMinSize / quoteMaxSize — пределы суммы «to place a market order» (у лимитного ордера не применяются).

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::GatewayResult;

use super::num;
use super::symbols::{STATUS_AUCTION, STATUS_OFF, STATUS_ONLINE, SymbolRules};

/// Разбор списка: пары с котируемой монетой quote (ключ — тикер монеты, baseCurrency). Пара с неразборчивым полем
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
        if num::text_opt(s, "quoteCurrency").as_deref() != Some(quote) {
            continue;
        }
        match parse(s) {
            Ok((ticker, r)) => {
                map.insert(ticker, r);
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

/// Разбор одной пары. Возвращает: (тикер монеты, правила). Ошибка BadResponse — нет обязательного поля.
pub fn parse(s: &Value) -> GatewayResult<(String, SymbolRules)> {
    let enabled = num::flag(s, "enableTrading")?;
    // Нет поля — аукциона нет (поле обязательное в спецификации, но отсутствие не делает пару опасной).
    let auction = matches!(s.get("callauctionIsEnabled"), Some(Value::Bool(true)));
    let status = match (enabled, auction) {
        (false, _) => STATUS_OFF,
        (true, true) => STATUS_AUCTION,
        (true, false) => STATUS_ONLINE,
    };
    let coef = |f: &str| num::dec_opt(s, f).map(|v| v.unwrap_or(Decimal::ONE));
    Ok((
        num::text(s, "baseCurrency")?,
        SymbolRules {
            symbol: num::text(s, "symbol")?,
            tick_size: num::dec(s, "priceIncrement")?,
            step_size: num::dec(s, "baseIncrement")?,
            min_qty: num::dec_opt(s, "baseMinSize")?,
            max_qty: num::dec_opt(s, "baseMaxSize")?,
            min_notional: num::dec_opt(s, "minFunds")?,
            status,
            price_limit_rate: num::dec_opt(s, "priceLimitRate")?,
            maker_coef: coef("makerFeeCoefficient")?,
            taker_coef: coef("takerFeeCoefficient")?,
            fee_currency: num::text_opt(s, "feeCurrency"),
        },
    ))
}
