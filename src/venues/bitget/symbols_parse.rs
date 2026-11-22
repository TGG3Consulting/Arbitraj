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

/// Ключом связи служит symbol биржи. isReality берётся из публичного справочника SPOT
/// (https://www.bitget.com/docs/catalog/market-market-data/market-instruments).
/// https://www.bitget.com/docs/uta/reality-trading-guide описывает отдельную торговлю rToken.
pub(super) fn classic_list(
    data: &Value,
    instruments: &Value,
    quote: &str,
) -> GatewayResult<(HashMap<String, SymbolRules>, Vec<String>)> {
    let rows = instruments
        .as_array()
        .filter(|rows| !rows.is_empty())
        .ok_or_else(|| super::errors::bad("instruments SPOT: нужен непустой список", ""))?;
    let mut reality = HashMap::new();
    for row in rows {
        let symbol = num::text(row, "symbol")?;
        if row["category"] != "SPOT"
            || reality.insert(symbol, row["isReality"].as_str()).is_some()
        {
            return Err(super::errors::bad(
                "instruments SPOT: неверная категория или повтор symbol",
                "",
            ));
        }
    }
    let (mut map, mut skipped) = parse_list(data, quote)?;
    if !map.is_empty()
        && !map
            .values()
            .any(|r| matches!(reality.get(&r.symbol), Some(Some("no" | "yes"))))
    {
        return Err(super::errors::bad(
            "instruments SPOT: класс не подтверждён ни для одной пары Classic",
            "",
        ));
    }
    map.retain(|_, rules| match reality.get(&rules.symbol) {
        Some(Some("no")) => true,
        Some(Some("yes")) => false,
        _ => {
            skipped.push(format!("{}: класс isReality не подтверждён", rules.symbol));
            false
        }
    });
    skipped.sort();
    Ok((map, skipped))
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Поля публичных ответов Bitget 22.11.2026, нужные разбору. Регистр baseCoin
    // Reality отличается от регистра symbol. RWAUSDT — обычная торгуемая пара.
    fn symbols() -> Value {
        let rows: Vec<_> = [
            ("RROLUSDT", "rROL", "2", "4", "10"),
            ("RWAUSDT", "RWA", "7", "2", "1"),
        ]
        .into_iter()
        .map(|(symbol, base, price, qty, minimum)| {
            json!({"symbol":symbol,"baseCoin":base,"quoteCoin":"USDT","status":"online",
                   "pricePrecision":price,"quantityPrecision":qty,"minTradeUSDT":minimum})
        })
        .collect();
        Value::Array(rows)
    }

    #[test]
    fn classic_subscription_excludes_reality_and_keeps_crypto() {
        use super::super::{pairs::Snapshot, symbols::Symbols};
        use crate::core::types::Asset;
        let rows = json!([
            {"category":"SPOT","symbol":"RROLUSDT","isReality":"yes"},
            {"category":"SPOT","symbol":"RWAUSDT","isReality":"no"}
        ]);
        let (map, skipped) = classic_list(&symbols(), &rows, "USDT").unwrap();
        assert!(skipped.is_empty());
        let directory = Symbols::default();
        directory.load(map, chrono::Utc::now());
        let assets = ["rROL", "RWA"].map(|ticker| Asset {
            chain: None,
            ticker: ticker.into(),
            contract: None,
        });
        let sub = Snapshot::build(&directory, &assets);
        assert_eq!(sub.books.len(), 1);
        assert_eq!(sub.books[0].1, "RWAUSDT");
        assert_eq!(sub.tickers, ["RWA"]);
        assert!(directory.symbol_of("rROL").is_none());
    }

    #[test]
    fn missing_class_is_reported_and_broken_catalog_rejected() {
        let regular = json!({"category":"SPOT","symbol":"RWAUSDT","isReality":"no"});
        let (map, skipped) = classic_list(&symbols(), &json!([regular]), "USDT").unwrap();
        assert!(map["RWA"].tradable());
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("RROLUSDT"));
        for broken in [
            json!([]),
            json!([regular, regular]),
            json!([{"category":"SPOT","symbol":"RROLUSDT"}]),
        ] {
            assert!(classic_list(&symbols(), &broken, "USDT").is_err());
        }
    }
}
