//! Файл: venues/okx/delist.rs
//! Что делает: делистинги OKX (решение 226, контракт delistings) — два источника:
//! (1) справочник инструментов: спот-пары с котируемой монетой venue.quote_asset, у которых задан expTime (время
//!    снятия пары, если её снимают вручную);
//! (2) анонсы делистинга (delist_ann.rs): «OKX to delist A, B and C spot trading pairs» — монеты из заголовка,
//!    которые есть в справочнике, блокируются на OKX с момента анонса.
//! Сбой анонсов не отменяет справочник — причина идёт в warnings ответа.
//! Кто вызывает: mod.rs (Venue::delistings), tests/delist_sources.rs.
//! От чего зависит: rest.rs, num.rs, errors.rs, ctx.rs, delist_ann.rs, gateway/delist_ann.rs, core/types.
//! Настройки: venue.quote_asset; delist.ann_max_age_h, delist.ann_max_pages (анонсы).
//!
//! Официальная документация: Get instruments GET /api/v5/public/instruments?instType=SPOT (20 запросов / 2 с по IP)
//! https://www.okx.com/docs-v5/en/ — «expTime String Expiry time Applicable to SPOT/MARGIN/FUTURES/SWAP/OPTION. … It
//! is the instrument offline time when there is SPOT/MARGIN/FUTURES/SWAP/ manual offline. Update once change.»
//! upcChg («Upcoming changes»: param tickSz, minSz, maxMktSz; newValue; effTime) про снятие пары не сообщает — не
//! используется. Живой ответ 08.11.2026: 1210 спот-инструментов, все state live, expTime и upcChg пусты у всех.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::{DelistItem, Delistings};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::num;

/// Источник для человека.
const SOURCE: &str = "OKX GET /api/v5/public/instruments: expTime";

/// Что делает: справочник инструментов (expTime) + анонсы делистинга. Возвращает: Delistings по фактам API.
/// Ошибки: связь или ответ OKX у справочника, не задана venue.quote_asset, неверный формат справочника.
pub async fn delistings(ctx: &Ctx) -> GatewayResult<Delistings> {
    let quote = ctx.text("venue.quote_asset")?;
    let q = vec![("instType", "SPOT".to_string())];
    let data = super::rest::public(ctx, "/api/v5/public/instruments", q).await?;
    let (base, known) = parse(&data, &quote)?;
    let ann = super::delist_ann::items(ctx, &known).await;
    Ok(crate::gateway::delist_ann::merge(base, ann))
}

/// Что делает: разбор data ответа инструментов. Принимает: data, котируемую монету. Возвращает: (пары с expTime >
/// 0, справочник «тикер → символ пары» с котируемой монетой quote — для сверки анонсов). Ошибка BadResponse — data
/// не массив, expTime не число, нет baseCcy/instId.
pub fn parse(data: &Value, quote: &str) -> GatewayResult<(Delistings, HashMap<String, String>)> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("instruments: data не массив", ""))?;
    let (mut items, mut known) = (vec![], HashMap::new());
    for s in list {
        if num::text_opt(s, "quoteCcy").as_deref() != Some(quote) {
            continue;
        }
        let ticker = num::text(s, "baseCcy")?;
        let symbol = num::text(s, "instId")?;
        known.insert(ticker.clone(), symbol.clone());
        let Some(raw) = num::text_opt(s, "expTime") else {
            continue;
        };
        let ms: i64 = raw
            .parse()
            .map_err(|_| super::errors::bad("expTime не число", &s.to_string()))?;
        if ms <= 0 {
            continue;
        }
        let at = DateTime::<Utc>::from_timestamp_millis(ms)
            .ok_or_else(|| super::errors::bad("expTime вне диапазона", &s.to_string()))?;
        items.push(DelistItem {
            ticker,
            symbol,
            trading_end: Some(at),
            withdraw_end: None,
            source: SOURCE.into(),
        });
    }
    Ok((Delistings::of(items), known))
}
