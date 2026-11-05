//! Файл: venues/phemex/symbols_parse.rs
//! Что делает: разбор ответа GET /public/products Phemex — спот-пары (products[] с type = Spot) в правила пар
//! (SymbolRules) и монеты (currencies[]) в масштабы valueScale.
//! Кто вызывает: symbols.rs (refresh), tests/phemex_real_data.rs.
//! От чего зависит: num.rs, symbols.rs (SymbolRules, статусы).
//! Настройки: нет (котируемую монету и «торгуемый» статус передаёт symbols.rs).
//!
//! Официально:
//! - Query product information GET /public/products (https://phemex-docs.github.io/#query-product-information): «Spot
//!   symbols are defined in .products[] with type=Spot», «Spot currencies are defined in .currencies[]». Полного
//!   перечня полей в текущей документации нет; имена — из таблиц «All Spot trading products / currencies» старой
//!   официальной документации (github.com/phemex/phemex-api-docs, Public-Spot-API-en.md): baseTickSizeEv,
//!   quoteTickSizeEv, minOrderValueEv, maxBaseOrderSizeEv, maxOrderValueEv; у монеты currency и valueScale («value scale
//!   factor»); priceScale — Price/Ratio/Value scales текущей документации. Имена baseCurrency и quoteCurrency — как в
//!   ордерах и сделках спота (Query trade history, wallet-order). Всё — сверить с живым ответом (урок 1, 2).
//! - Масштабы (#price-ratio-value-scales): «priceScale follows the valueScale of quote-currency», «the scaled price …
//!   must be multiple times of quoteTickSizeEv», «qtyScale follows the valueScale of base-currency … scaled qty must be
//!   multiple times of baseTickSizeEv». Сумма ордера (minOrderValueEv, maxOrderValueEv) — в масштабе котируемой монеты
//!   (в таблице: «10 USDT» = 1000000000 при valueScale USDT 8).
//! - Поле статуса пары в документации не описано (поле status есть только в примерах CCXT, не официальный источник):
//!   торгуемой считается пара, у которой status равен venue.pair_status_listed (значение — по живому ответу).

use std::collections::HashMap;

use serde_json::Value;

use crate::gateway::errors::GatewayResult;

use super::num;
use super::symbols::{STATUS_LISTED, STATUS_NO_FIELD, STATUS_OTHER, SymbolRules};

/// Разобранный справочник: пары по тикеру базовой монеты, масштабы монет, пропущенные «символ: причина».
pub struct Parsed {
    pub pairs: HashMap<String, SymbolRules>,
    pub scales: HashMap<String, u32>,
    pub skipped: Vec<String>,
}

/// Что делает: разбор data /public/products. Принимает: data, котируемую монету, торгуемый статус пары.
/// Возвращает: Parsed. Ошибка BadResponse — нет массивов products или currencies.
pub fn parse_products(data: &Value, quote: &str, listed: &str) -> GatewayResult<Parsed> {
    let mut scales = HashMap::new();
    let mut skipped = Vec::new();
    for c in num::arr(data, "currencies")? {
        let name = num::text_opt(c, "currency").unwrap_or_default();
        match num::scale_of(c, "valueScale") {
            Ok(s) if !name.is_empty() => {
                scales.insert(name, s);
            }
            Ok(_) => skipped.push("монета без currency".to_string()),
            Err(e) => skipped.push(format!("{name}: {}", e.message)),
        }
    }
    let mut pairs = HashMap::new();
    for p in num::arr(data, "products")? {
        if num::text_opt(p, "type").as_deref() != Some("Spot")
            || num::text_opt(p, "quoteCurrency").as_deref() != Some(quote)
        {
            continue;
        }
        match parse(p, &scales, listed) {
            Ok(r) => {
                pairs.insert(r.base.clone(), r);
            }
            Err(e) => skipped.push(format!(
                "{}: {}",
                num::text_opt(p, "symbol").unwrap_or_default(),
                e.message
            )),
        }
    }
    Ok(Parsed {
        pairs,
        scales,
        skipped,
    })
}

/// Разбор одной пары. Принимает: пару, масштабы монет, торгуемый статус. Возвращает: правила.
/// Ошибка BadResponse — нет обязательного поля, нет масштаба монеты, шаг не больше нуля.
pub fn parse(p: &Value, scales: &HashMap<String, u32>, listed: &str) -> GatewayResult<SymbolRules> {
    let base = num::text(p, "baseCurrency")?;
    let quote = num::text(p, "quoteCurrency")?;
    let scale = |c: &str| {
        scales
            .get(c)
            .copied()
            .ok_or_else(|| super::errors::bad(&format!("нет valueScale монеты {c}"), ""))
    };
    let (base_scale, quote_scale) = (scale(&base)?, scale(&quote)?);
    let price_scale = num::scale_of(p, "priceScale")?;
    let positive = |f: &str| {
        let v = num::int(p, f)?;
        i64::try_from(v)
            .ok()
            .filter(|v| *v > 0)
            .ok_or_else(|| super::errors::bad(&format!("{f} = {v} не больше нуля"), ""))
    };
    let opt = |f: &str, s: u32| num::scaled_opt(p, f, s);
    let status = match num::text_opt(p, "status") {
        Some(s) if s == listed => STATUS_LISTED,
        Some(_) => STATUS_OTHER,
        None => STATUS_NO_FIELD,
    };
    let (tick_ep, step_ev) = (positive("quoteTickSizeEv")?, positive("baseTickSizeEv")?);
    let unscale = |v: i64, s: u32| num::unscale(i128::from(v), s).unwrap_or_default();
    Ok(SymbolRules {
        symbol: num::text(p, "symbol")?,
        base,
        quote,
        price_scale,
        base_scale,
        quote_scale,
        tick_ep,
        step_ev,
        tick_size: unscale(tick_ep, price_scale),
        step_size: unscale(step_ev, base_scale),
        min_notional: opt("minOrderValueEv", quote_scale)?,
        max_notional: opt("maxOrderValueEv", quote_scale)?,
        max_qty: opt("maxBaseOrderSizeEv", base_scale)?,
        status,
        status_text: num::text_opt(p, "status").unwrap_or_default(),
    })
}
