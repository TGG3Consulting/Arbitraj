//! Файл: venues/bingx/delist.rs
//! Что делает: делистинги BingX (решение 175, контракт delistings) — пары с котируемой монетой venue.quote_asset в
//! статусе 29 «pre-delisted» справочника пар (symbols.rs, без отдельного запроса).
//! Кто вызывает: mod.rs (Venue::delistings), tests/delist_parse.rs.
//! От чего зависит: symbols.rs, ctx.rs, core/types (Delistings).
//! Настройки: venue.quote_asset, venue.symbols_refresh_s (через symbols.rs).
//!
//! Официальная документация: «Spot trading symbols» GET /openApi/spot/v1/common/symbols
//! https://bingx-api.github.io/docs-v3/#/en/Spot/Market%20Data/Spot%20trading%20symbols — status: 0 offline, 1 online,
//! 5 pre-open, 10 accessed, 25 suspended, 29 pre-delisted, 30 delisted. Времени делистинга в документации нет: запись
//! без времени (trading_end = None) — ядро такую сделку не делает («время не сообщено»). Статус 30 «delisted» — пара
//! уже снята; её отсекает общая страховка «пара не торгуется» (core/presence.rs), здесь не повторяется.
//! Поле offTime в ответе есть (живая запись 2026-10-24: SKYAI-USDT offTime 1753412400000 в прошлом при status 1 и
//! timeOnline 1777551300000), его смысл в документации не описан — не используется (вопрос Тиграну).

use crate::core::types::{DelistItem, Delistings};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::symbols::Symbols;

/// Статус «pre-delisted» по документации docs-v3.
pub const STATUS_PRE_DELISTED: i64 = 29;

/// Источник для человека.
const SOURCE: &str =
    "BingX GET /openApi/spot/v1/common/symbols: status 29 pre-delisted (время не сообщается)";

/// Что делает: пары в статусе pre-delisted по свежему справочнику пар. Возвращает: Delistings по фактам API.
/// Ошибки: настройки, связь, ответ BingX.
pub async fn delistings(ctx: &Ctx, symbols: &Symbols) -> GatewayResult<Delistings> {
    symbols.ensure_fresh(ctx).await?;
    Ok(build(symbols.with_status(STATUS_PRE_DELISTED)))
}

/// Что делает: записи делистинга из пар (тикер, символ) статуса pre-delisted. Ошибок нет.
pub fn build(pairs: Vec<(String, String)>) -> Delistings {
    let items = pairs
        .into_iter()
        .map(|(ticker, symbol)| DelistItem {
            ticker,
            symbol,
            trading_end: None,
            withdraw_end: None,
            source: SOURCE.into(),
        })
        .collect();
    Delistings::of(items)
}
