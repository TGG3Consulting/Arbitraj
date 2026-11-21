//! Файл: venues/bitget/delist.rs
//! Что делает: делистинги Bitget (решение 226, контракт delistings) — два источника: (1) пары с котируемой монетой
//! venue.quote_asset, у которых в справочнике пар задано время снятия offTime; (2) анонсы делистинга (delist_ann.rs,
//! уточнение Тиграна 08.11.2026: «непокрытого быть не должно») — пары и время со страницы анонса. Сбой анонсов не
//! отменяет справочник — причина идёт в warnings ответа.
//! Кто вызывает: mod.rs (Venue::delistings), tests/delist_sources.rs.
//! От чего зависит: rest.rs, num.rs, errors.rs, ctx.rs, delist_ann.rs, gateway/delist_ann.rs, core/types.
//! Настройки: venue.quote_asset; delist.ann_max_age_h, delist.ann_max_pages, delist.ann_html_per_refresh (анонсы).
//!
//! Официальная документация: Get Symbol Info GET /api/v2/spot/public/symbols («Frequency limit: 20 times/1s (IP)»)
//! https://www.bitget.com/docs/catalog/classic-spot-market/classic-spot-market#get-symbol-info — «offTime string
//! Symbol off time, e.g: 1744797600000» (мс). Пусто — снятия нет. Живая сверка 08.11.2026: у 2 пар из 3668
//! (ALPINEUSDT, QUBICUSDT) offTime 1794564000000 = 13.11.2026 10:00 UTC, и в тот же день анонс Bitget GET
//! /api/v2/public/annoucements?annType=symbol_delisting: «Notice of Delisting 2 Spot Trading Pairs on November 13,
//! 2026» — поле = время делистинга. Анонсы Bitget тикеров в заголовке не содержат — пары и время берутся со
//! страницы анонса (delist_ann.rs).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::{DelistItem, Delistings};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::num;

/// Источник для человека.
const SOURCE: &str = "Bitget GET /api/v2/spot/public/symbols: offTime";

/// Что делает: справочник пар (offTime) + анонсы делистинга. Возвращает: Delistings по фактам API.
/// Ошибки: связь или ответ Bitget у справочника, не задана venue.quote_asset, неверный формат справочника.
pub async fn delistings(ctx: &Ctx) -> GatewayResult<Delistings> {
    let quote = ctx.text("venue.quote_asset")?;
    let data = super::rest::public(ctx, "/api/v2/spot/public/symbols", vec![]).await?;
    let (base, known) = parse(&data, &quote)?;
    let ann = super::delist_ann::items(ctx, &known, &quote).await;
    Ok(crate::gateway::delist_ann::merge(base, ann))
}

/// Что делает: разбор data ответа. Принимает: data, котируемую монету. Возвращает: (пары с котируемой монетой quote и
/// offTime > 0, справочник «тикер → символ» с котируемой монетой quote — для сверки анонсов). Ошибка BadResponse —
/// data не массив, offTime не число, нет baseCoin/symbol.
pub fn parse(data: &Value, quote: &str) -> GatewayResult<(Delistings, HashMap<String, String>)> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("spot/public/symbols: data не массив", ""))?;
    let (mut items, mut known) = (vec![], HashMap::new());
    for s in list {
        if num::text_opt(s, "quoteCoin").as_deref() != Some(quote) {
            continue;
        }
        let ticker = num::text(s, "baseCoin")?;
        let symbol = num::text(s, "symbol")?;
        known.insert(ticker.clone(), symbol.clone());
        let Some(raw) = num::text_opt(s, "offTime") else {
            continue;
        };
        let ms: i64 = raw
            .parse()
            .map_err(|_| super::errors::bad("offTime не число", &s.to_string()))?;
        if ms <= 0 {
            continue;
        }
        let at = DateTime::<Utc>::from_timestamp_millis(ms)
            .ok_or_else(|| super::errors::bad("offTime вне диапазона", &s.to_string()))?;
        items.push(DelistItem {
            ticker,
            symbol,
            trading_end: Some(at),
            withdraw_end: None,
            source: SOURCE.into(),
            st: false,
        });
    }
    Ok((Delistings::of(items), known))
}
