//! Файл: venues/gate/delist.rs
//! Что делает: делистинги Gate (решение 175, контракт delistings) — пары с котируемой монетой venue.quote_asset, у
//! которых в справочнике пар задано время делистинга.
//! Кто вызывает: mod.rs (Venue::delistings), tests/delist_parse.rs.
//! От чего зависит: rest.rs, num.rs, errors.rs, ctx.rs, core/types (Delistings).
//! Настройки: venue.quote_asset.
//!
//! Официальная документация: API v4 → Spot → «Query all supported currency pairs» (GET /spot/currency_pairs)
//! https://www.gate.com/docs/developers/apiv4/en/ ; модель CurrencyPair (SDK
//! https://github.com/gate/gateapi-python/blob/master/docs/CurrencyPair.md): delisting_time — «Expected delisting time
//! as a Unix timestamp in seconds», base, quote, id. Времени закрытия вывода в модели нет. Поле отсутствует или 0 —
//! делистинга нет (в записанном ответе 2026-11-04 у 2154 пар из 2188 поля нет, у 34 — время в будущем).

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::{DelistItem, Delistings};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::num;

/// Источник для человека.
const SOURCE: &str = "Gate GET /spot/currency_pairs: delisting_time";

/// Что делает: запрашивает справочник пар и выбирает пары в делистинге. Возвращает: Delistings по фактам API.
/// Ошибки: связь, ответ Gate, не задана venue.quote_asset, неверный формат.
pub async fn delistings(ctx: &Ctx) -> GatewayResult<Delistings> {
    let quote = ctx.text("venue.quote_asset")?;
    let info = super::rest::public_heavy(ctx, "/spot/currency_pairs", vec![]).await?;
    parse(&info, &quote)
}

/// Что делает: разбор ответа GET /spot/currency_pairs. Принимает: ответ, котируемую монету. Возвращает: пары с
/// котируемой монетой quote и delisting_time > 0. Ошибка BadResponse — ответ не массив или неверное время.
pub fn parse(data: &Value, quote: &str) -> GatewayResult<Delistings> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("currency_pairs не массив", &data.to_string()))?;
    let mut items = vec![];
    for s in list {
        if num::text_opt(s, "quote").as_deref() != Some(quote) {
            continue;
        }
        let Some(raw) = num::text_opt(s, "delisting_time") else {
            continue;
        };
        let secs: i64 = raw
            .parse()
            .map_err(|_| super::errors::bad("delisting_time не число", &s.to_string()))?;
        if secs <= 0 {
            continue;
        }
        let at = DateTime::<Utc>::from_timestamp(secs, 0)
            .ok_or_else(|| super::errors::bad("delisting_time вне диапазона", &s.to_string()))?;
        items.push(DelistItem {
            ticker: num::text(s, "base")?,
            symbol: num::text(s, "id")?,
            trading_end: Some(at),
            withdraw_end: None,
            source: SOURCE.into(),
            st: false,
        });
    }
    Ok(Delistings::of(items))
}
