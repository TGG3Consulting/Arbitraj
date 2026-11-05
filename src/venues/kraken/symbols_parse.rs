//! Файл: venues/kraken/symbols_parse.rs
//! Что делает: разбор ответа GET /0/public/AssetPairs Kraken в правила пар (SymbolRules).
//! Кто вызывает: symbols.rs (refresh), tests/kraken_real_data.rs.
//! От чего зависит: num.rs, symbols.rs (SymbolRules, статусы).
//! Настройки: нет (котируемую монету передаёт symbols.rs).
//!
//! Официально — OpenAPI https://docs.kraken.com/openapi/spot-rest.yaml, схема pairs (GET /0/public/AssetPairs): result —
//! объект «имя пары → описание»; altname, wsname («WebSocket pair name»), base / quote (имена активов Kraken), pair_decimals
//! (знаки цены), lot_decimals (знаки количества), cost_decimals, ordermin («Minimum order size (in terms of base
//! currency)»), costmin («Minimum order cost (in terms of quote currency)»), tick_size («Minimum increment between valid
//! price levels»), status (online, cancel_only, post_only, limit_only, reduce_only). Шаг количества — 10^−lot_decimals.
//! Флагов «покупка/продажа через API» нет: торгуется = status online. С 08.09.2026 поля fees/fees_maker всегда [] —
//! ставки только из TradeVolume (fees.rs). Ключ пары в справочнике — имя актива base (как в Balance и Funding —
//! сверить на живом ответе: у старых монет имена вида XXBT/ZUSD).

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::GatewayResult;

use super::num;
use super::symbols::{STATUS_OFF, STATUS_ONLINE, STATUS_OTHER, SymbolRules};

/// Разбор result AssetPairs: пары с котируемым активом quote (ключ — base). Пара с неразборчивым полем не валит весь
/// справочник, а попадает в skipped («имя пары: причина»). Ошибка BadResponse — result не объект.
pub fn parse_list(
    result: &Value,
    quote: &str,
) -> GatewayResult<(HashMap<String, SymbolRules>, Vec<String>)> {
    let list = result
        .as_object()
        .ok_or_else(|| super::errors::bad("AssetPairs: result не объект", ""))?;
    let (mut map, mut skipped) = (HashMap::new(), Vec::new());
    for (name, s) in list {
        if num::text_opt(s, "quote").as_deref() != Some(quote) {
            continue;
        }
        match parse(name, s) {
            Ok((base, r)) => {
                map.insert(base, r);
            }
            Err(e) => skipped.push(format!("{name}: {}", e.message)),
        }
    }
    Ok((map, skipped))
}

/// Разбор одной пары. Принимает: имя пары (ключ result), описание. Возвращает: (актив base, правила).
/// Ошибка BadResponse — нет обязательного поля.
pub fn parse(name: &str, s: &Value) -> GatewayResult<(String, SymbolRules)> {
    let state = num::text(s, "status")?;
    let status = match state.as_str() {
        "online" => STATUS_ONLINE,
        "cancel_only" | "reduce_only" => STATUS_OFF,
        _ => STATUS_OTHER,
    };
    let lot = num::int(s, "lot_decimals")?;
    let price_dp = num::int(s, "pair_decimals")?;
    let to_dp = |v: i64| u32::try_from(v).ok().filter(|v| *v <= 28);
    let lot = to_dp(lot).ok_or_else(|| super::errors::bad("lot_decimals вне 0..28", name))?;
    let price_dp =
        to_dp(price_dp).ok_or_else(|| super::errors::bad("pair_decimals вне 0..28", name))?;
    Ok((
        num::text(s, "base")?,
        SymbolRules {
            symbol: name.to_string(),
            wsname: num::text(s, "wsname")?,
            tick_size: num::dec(s, "tick_size")?,
            step_size: Decimal::new(1, lot),
            lot_decimals: lot,
            price_decimals: price_dp,
            min_qty: num::dec_opt(s, "ordermin")?,
            min_cost: num::dec_opt(s, "costmin")?,
            status,
            state,
        },
    ))
}
