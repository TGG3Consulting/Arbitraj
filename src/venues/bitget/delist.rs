//! Файл: venues/bitget/delist.rs
//! Что делает: делистинги Bitget (решение 226, контракт delistings) — пары с котируемой монетой venue.quote_asset, у
//! которых в справочнике пар задано время снятия offTime.
//! Кто вызывает: mod.rs (Venue::delistings), tests/delist_sources.rs.
//! От чего зависит: rest.rs, num.rs, errors.rs, ctx.rs, core/types (Delistings).
//! Настройки: venue.quote_asset.
//!
//! Официальная документация: Get Symbol Info GET /api/v2/spot/public/symbols («Frequency limit: 20 times/1s (IP)»)
//! https://www.bitget.com/docs/catalog/classic-spot-market/classic-spot-market#get-symbol-info — «offTime string
//! Symbol off time, e.g: 1744797600000» (мс). Пусто — снятия нет. Живая сверка 08.11.2026: у 2 пар из 3668
//! (ALPINEUSDT, QUBICUSDT) offTime 1794564000000 = 13.11.2026 10:00 UTC, и в тот же день анонс Bitget GET
//! /api/v2/public/annoucements?annType=symbol_delisting: «Notice of Delisting 2 Spot Trading Pairs on November 13,
//! 2026» — поле = время делистинга. Анонсы Bitget тикеров в заголовке не содержат — не разбираются.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::{DelistItem, Delistings};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::num;

/// Источник для человека.
const SOURCE: &str = "Bitget GET /api/v2/spot/public/symbols: offTime";

/// Что делает: запрашивает справочник пар и выбирает пары с offTime. Возвращает: Delistings по фактам API.
/// Ошибки: связь, ответ Bitget, не задана venue.quote_asset, неверный формат.
pub async fn delistings(ctx: &Ctx) -> GatewayResult<Delistings> {
    let quote = ctx.text("venue.quote_asset")?;
    let data = super::rest::public(ctx, "/api/v2/spot/public/symbols", vec![]).await?;
    parse(&data, &quote)
}

/// Что делает: разбор data ответа. Принимает: data, котируемую монету. Возвращает: пары с котируемой монетой quote и
/// offTime > 0. Ошибка BadResponse — data не массив или offTime не число.
pub fn parse(data: &Value, quote: &str) -> GatewayResult<Delistings> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("spot/public/symbols: data не массив", ""))?;
    let mut items = vec![];
    for s in list {
        if num::text_opt(s, "quoteCoin").as_deref() != Some(quote) {
            continue;
        }
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
            ticker: num::text(s, "baseCoin")?,
            symbol: num::text(s, "symbol")?,
            trading_end: Some(at),
            withdraw_end: None,
            source: SOURCE.into(),
        });
    }
    Ok(Delistings::of(items))
}
