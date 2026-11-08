//! Файл: venues/htx/delist.rs
//! Что делает: делистинги HTX (решение 226, контракт delistings) — пары с котируемой монетой venue.quote_asset, ещё не
//! снятые (state не offline), у которых справочник ставит признак делистинга dl = true; время — tca («the time trade
//! close at»).
//! Кто вызывает: mod.rs (Venue::delistings), tests/delist_sources.rs.
//! От чего зависит: rest.rs, num.rs, errors.rs, ctx.rs (ticker), core/types (Delistings).
//! Настройки: venue.quote_asset.
//!
//! Официальная документация: Get all Supported Trading Symbol (V1) GET /v1/settings/common/symbols
//! https://huobiapi.github.io/docs/spot/v1/en/ — «dl delist boolean delist», «tca trade-close-at long the time trade
//! close at», «state … unknown，not-online，pre-online，online，suspend，offline，transfer-board，fuse». Справочник
//! торговли адаптера (GET /v2/settings/common/symbols, symbols.rs) этих полей не несёт — отдельный запрос.
//! Живой ответ 08.11.2026: 2067 пар; dl = true у всех 1469 offline и у 4 suspend (eurrusdt, wlfiusdt, usdrusdt,
//! velodromeusdt — tca в прошлом); у online dl = false и tca = 1893470400000 (01.01.2030) почти у всех, у altusdt —
//! 1871776800000 при dl = false. Пара с dl = false в делистинг не попадает (что значит tca без dl, документация не
//! говорит — вопрос Тиграну). dl = true без tca — запись без времени (ядро не торгует: «время не сообщено»).

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::{DelistItem, Delistings};
use crate::gateway::errors::GatewayResult;

use super::ctx::{Ctx, ticker};
use super::num;

/// Источник для человека.
const SOURCE: &str = "HTX GET /v1/settings/common/symbols: dl = true, tca";

/// Что делает: запрашивает справочник пар V1 и выбирает пары с dl = true. Возвращает: Delistings по фактам API.
/// Ошибки: связь, ответ HTX, не задана venue.quote_asset, неверный формат.
pub async fn delistings(ctx: &Ctx) -> GatewayResult<Delistings> {
    let quote = ctx.text("venue.quote_asset")?;
    let data = super::rest::public(ctx, "/v1/settings/common/symbols", vec![]).await?;
    parse(&data, &quote)
}

/// Что делает: разбор data ответа. Принимает: data, котируемую монету (тикер ядра, «USDT»). Возвращает: пары с
/// котируемой монетой quote, state не offline и dl = true. Ошибка BadResponse — data не массив, нет bc/symbol.
pub fn parse(data: &Value, quote: &str) -> GatewayResult<Delistings> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("v1 common/symbols: data не массив", ""))?;
    let mut items = vec![];
    for s in list {
        if num::text_opt(s, "qc").map(|q| ticker(&q)).as_deref() != Some(quote)
            || num::text_opt(s, "state").as_deref() == Some("offline")
            || !matches!(s.get("dl"), Some(Value::Bool(true)))
        {
            continue;
        }
        let end = s
            .get("tca")
            .and_then(Value::as_i64)
            .filter(|ms| *ms > 0)
            .and_then(DateTime::<Utc>::from_timestamp_millis);
        items.push(DelistItem {
            ticker: ticker(&num::text(s, "bc")?),
            symbol: num::text(s, "symbol")?,
            trading_end: end,
            withdraw_end: None,
            source: SOURCE.into(),
        });
    }
    Ok(Delistings::of(items))
}
