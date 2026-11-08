//! Файл: venues/bybit/delist.rs
//! Что делает: делистинги Bybit (решение 226, контракт delistings) — два источника:
//! (1) справочник спот-пар: пары с котируемой монетой venue.quote_asset и меткой stTag = "1" (special treatment) —
//!    без времени, ядро монету на Bybit не торгует;
//! (2) анонсы делистинга (delist_ann.rs): «Delisting of A, B and C» с тегом Spot — монеты из заголовка, которые есть
//!    в справочнике, блокируются на Bybit с момента анонса.
//! Сбой анонсов не отменяет справочник — причина идёт в warnings ответа.
//! Кто вызывает: mod.rs (Venue::delistings), tests/delist_sources.rs.
//! От чего зависит: rest.rs, num.rs, errors.rs, ctx.rs, delist_ann.rs, gateway/delist_ann.rs, core/types.
//! Настройки: venue.quote_asset; delist.ann_max_age_h, delist.ann_max_pages (анонсы).
//!
//! Официальная документация: Get Instruments Info GET /v5/market/instruments-info?category=spot
//! https://bybit-exchange.github.io/docs/v5/market/instrument — «stTag string Whether or not it has an special
//! treatment label. 0: false, 1: true»; status у спота — только Trading (времени делистинга нет). Метка — у ПАРЫ:
//! живой ответ 08.11.2026 — 521 пара, stTag "1" у 13 (FTTUSDT, CTCUSDT, …), при этом DYDXUSDC "1", а DYDXUSDT "0";
//! поэтому берутся только пары с venue.quote_asset.

use std::collections::HashMap;

use serde_json::Value;

use crate::core::types::{DelistItem, Delistings};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::num;

/// Источник для человека.
const SOURCE: &str =
    "Bybit GET /v5/market/instruments-info (spot): stTag = 1 (special treatment — риск делистинга)";

/// Что делает: справочник спот-пар (stTag) + анонсы делистинга. Возвращает: Delistings по фактам API.
/// Ошибки: связь или ответ Bybit у справочника, не задана venue.quote_asset, неверный формат справочника.
pub async fn delistings(ctx: &Ctx) -> GatewayResult<Delistings> {
    let quote = ctx.text("venue.quote_asset")?;
    let p = vec![("category", "spot".to_string())];
    let info = super::rest::public(ctx, "/v5/market/instruments-info", p).await?;
    let (base, known) = parse(&info, &quote)?;
    let ann = super::delist_ann::items(ctx, &known).await;
    Ok(crate::gateway::delist_ann::merge(base, ann))
}

/// Что делает: разбор result ответа. Принимает: result ({list: [...]}), котируемую монету. Возвращает: (пары со
/// stTag "1", справочник «тикер → символ» с котируемой монетой quote — для сверки анонсов). Ошибка BadResponse — нет
/// list, нет baseCoin/symbol.
pub fn parse(info: &Value, quote: &str) -> GatewayResult<(Delistings, HashMap<String, String>)> {
    let list = info
        .get("list")
        .and_then(Value::as_array)
        .ok_or_else(|| super::errors::bad("instruments-info: нет list", ""))?;
    let (mut items, mut known) = (vec![], HashMap::new());
    for s in list {
        if num::text_opt(s, "quoteCoin").as_deref() != Some(quote) {
            continue;
        }
        let ticker = num::text(s, "baseCoin")?;
        let symbol = num::text(s, "symbol")?;
        known.insert(ticker.clone(), symbol.clone());
        if num::text_opt(s, "stTag").as_deref() != Some("1") {
            continue;
        }
        items.push(DelistItem {
            ticker,
            symbol,
            trading_end: None,
            withdraw_end: None,
            source: SOURCE.into(),
        });
    }
    Ok((Delistings::of(items), known))
}
