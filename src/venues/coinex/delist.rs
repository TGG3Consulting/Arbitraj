//! Файл: venues/coinex/delist.rs
//! Что делает: делистинги CoinEx (решение 175, контракт delistings) — пары с котируемой монетой venue.quote_asset, у
//! которых в справочнике пар задано ожидаемое время делистинга.
//! Кто вызывает: mod.rs (Venue::delistings), tests/delist_parse.rs.
//! От чего зависит: rest.rs, num.rs, errors.rs, ctx.rs, core/types (Delistings).
//! Настройки: venue.quote_asset.
//!
//! Официальная документация: Get Market Status (GET /spot/market, база /v2)
//! https://docs.coinex.com/api/v2/spot/market/http/list-market: delisted_at (int) — «Expected time to be delisted, in
//! milliseconds», market, base_ccy, quote_ccy. Времени закрытия вывода нет. Поле отсутствует, null или 0 — делистинга
//! нет. В примере ответа документации delisted_at = 1819704600000 у рынка со status «online» — значит, время может
//! стоять у торгуемой пары заранее; ядро сравнивает его с порогом delist.block_hours.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::{DelistItem, Delistings};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::num;

/// Источник для человека.
const SOURCE: &str = "CoinEx GET /v2/spot/market: delisted_at";

/// Что делает: запрашивает справочник пар и выбирает пары в делистинге. Возвращает: Delistings по фактам API.
/// Ошибки: связь, ответ CoinEx, не задана venue.quote_asset, неверный формат.
pub async fn delistings(ctx: &Ctx) -> GatewayResult<Delistings> {
    let quote = ctx.text("venue.quote_asset")?;
    let data = super::rest::public(ctx, "/v2/spot/market", vec![]).await?;
    parse(&data, &quote)
}

/// Что делает: разбор data ответа GET /v2/spot/market. Принимает: data, котируемую монету. Возвращает: пары с
/// котируемой монетой quote и delisted_at > 0. Ошибка BadResponse — data не массив или неверное время.
pub fn parse(data: &Value, quote: &str) -> GatewayResult<Delistings> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("spot/market: data не массив", &data.to_string()))?;
    let mut items = vec![];
    for s in list {
        if num::text_opt(s, "quote_ccy").as_deref() != Some(quote) {
            continue;
        }
        let Some(raw) = num::text_opt(s, "delisted_at") else {
            continue;
        };
        let ms: i64 = raw
            .parse()
            .map_err(|_| super::errors::bad("delisted_at не число", &s.to_string()))?;
        if ms <= 0 {
            continue;
        }
        let at = DateTime::<Utc>::from_timestamp_millis(ms)
            .ok_or_else(|| super::errors::bad("delisted_at вне диапазона", &s.to_string()))?;
        items.push(DelistItem {
            ticker: num::text(s, "base_ccy")?,
            symbol: num::text(s, "market")?,
            trading_end: Some(at),
            withdraw_end: None,
            source: SOURCE.into(),
            st: false,
        });
    }
    Ok(Delistings::of(items))
}
