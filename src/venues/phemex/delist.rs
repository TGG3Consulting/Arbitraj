//! Файл: venues/phemex/delist.rs
//! Что делает: делистинги Phemex (решение 226, контракт delistings) — торгуемые спот-пары (status Listed) с
//! котируемой монетой venue.quote_asset, у которых задано время делистинга timeline[3].
//! Кто вызывает: mod.rs (Venue::delistings), tests/delist_sources.rs.
//! От чего зависит: rest.rs, num.rs, errors.rs, ctx.rs, core/types (Delistings).
//! Настройки: venue.quote_asset.
//!
//! Официальная документация: Query product information plus GET /public/products-plus
//! (https://phemex-docs.github.io/#query-product-information-plus) — «Spot symbols are defined in .products[] with
//! type Spot. list time is defined in timeline[1]. delist time is defined in timeline[3].» Единицы — мс (живой ответ
//! 08.11.2026: у снятых пар timeline[3] = 1789120800000 и т. п.; у торгуемых — 0). Пара уже снята (status Delisted) —
//! её отсекает общая страховка «пара не торгуется» (core/presence.rs), здесь не повторяется.
//! Живой ответ 08.11: 176 пар Listed, у 170 timeline[3] = 0, у 6 (VOXEL, FIS, FOXY, ELX, SKATE, PIGGY) — время
//! декабря 2025, а пара всё ещё Listed: ядро такую запись читает как «время уже прошло» и монету на Phemex не торгует
//! (не торгуем вслепую). Поле stStatus в ответе есть (у SUSHI, GRASS = 1), в документации его нет — не используется
//! (вопрос Тиграну).

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::{DelistItem, Delistings};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::num;

/// Источник для человека.
const SOURCE: &str = "Phemex GET /public/products-plus: timeline[3] (delist time)";

/// Что делает: запрашивает справочник продуктов и выбирает спот-пары с временем делистинга. Возвращает: Delistings
/// по фактам API. Ошибки: связь, ответ Phemex, не задана venue.quote_asset, неверный формат.
pub async fn delistings(ctx: &Ctx) -> GatewayResult<Delistings> {
    let quote = ctx.text("venue.quote_asset")?;
    let data = super::rest::public(ctx, "/public/products-plus", vec![], true).await?;
    parse(&data, &quote)
}

/// Что делает: разбор data ответа. Принимает: data ({products: [...]}), котируемую монету. Возвращает: пары Spot,
/// Listed, с котируемой монетой quote и timeline[3] > 0. Ошибка BadResponse — нет products, у пары нет timeline
/// из 4+ чисел.
pub fn parse(data: &Value, quote: &str) -> GatewayResult<Delistings> {
    let list = data
        .get("products")
        .and_then(Value::as_array)
        .ok_or_else(|| super::errors::bad("products-plus: нет массива products", ""))?;
    let mut items = vec![];
    for p in list {
        if num::text_opt(p, "type").as_deref() != Some("Spot")
            || num::text_opt(p, "status").as_deref() != Some("Listed")
            || num::text_opt(p, "quoteCurrency").as_deref() != Some(quote)
        {
            continue;
        }
        let ms = p
            .get("timeline")
            .and_then(Value::as_array)
            .and_then(|t| t.get(3))
            .and_then(Value::as_i64)
            .ok_or_else(|| super::errors::bad("нет timeline[3]", &p.to_string()))?;
        if ms <= 0 {
            continue;
        }
        let at = DateTime::<Utc>::from_timestamp_millis(ms)
            .ok_or_else(|| super::errors::bad("timeline[3] вне диапазона", &p.to_string()))?;
        items.push(DelistItem {
            ticker: num::text(p, "baseCurrency")?,
            symbol: num::text(p, "symbol")?,
            trading_end: Some(at),
            withdraw_end: None,
            source: SOURCE.into(),
        });
    }
    Ok(Delistings::of(items))
}
