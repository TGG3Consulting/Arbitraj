//! Файл: core/pairs_fresh.rs
//! Что делает: можно ли верить справочнику пар биржи (MarketEvent::SpotPairs) — он не старше порога
//! venue.symbols_stale_s (обновление у биржи может падать, тогда биржа присылает прежний справочник с прежним
//! loaded_at); и проверка стороны сделки по нему перед покупкой: на A покупка через API разрешена, на B — продажа.
//! Не задан порог, справочник не пришёл или устарел — по паре не торгуем (не торгуем вслепую).
//! Кто вызывает: rebuild.rs (свежесть по каждой бирже для справочника — presence.rs), precheck.rs (сторона сделки).
//! От чего зависит: market_state.rs, settings, core/types.
//! Настройки: venue.symbols_stale_s (по площадке).

use chrono::{DateTime, Utc};

use crate::core::types::VenueId;
use crate::settings::read::At;
use crate::settings::store::Store;

use super::market_state::MarketState;

/// Порог устаревания справочника пар площадки (venue.symbols_stale_s). Ошибка — текст: не задан.
pub fn limit(settings: &Store, venue: &VenueId) -> Result<u64, String> {
    let at = At {
        venue: Some(&venue.0),
        coin: None,
    };
    settings
        .integer("venue.symbols_stale_s", at)
        .map_err(|e| format!("порог устаревания справочника пар: {}", e.message))
}

/// Свежесть справочника пар биржи. Принимает: рынок, биржу, порог в секундах (или почему его нет), «сейчас».
/// Возвращает: Ok — справочнику можно верить; Err — текст причины (порог не задан, справочника нет, устарел).
pub fn check(
    market: &MarketState,
    venue: &VenueId,
    limit: &Result<u64, String>,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let limit = limit.as_ref().map_err(Clone::clone)?;
    let at = market
        .pairs_loaded_at(venue)
        .ok_or_else(|| format!("{}: справочник пар ещё не получен", venue.0))?;
    let age = (now - at).num_seconds().max(0) as u64;
    if age > *limit {
        return Err(format!(
            "{}: справочник пар устарел — получен {age} с назад, порог {limit} с (обновление не проходит, см. журнал)",
            venue.0
        ));
    }
    Ok(())
}

/// Сторона сделки на бирже по справочнику пар (предпроверка, precheck.rs): пара есть и торгуется по статусу,
/// справочник свежий, через API разрешена покупка (buy = true, площадка A) или продажа (площадка B).
/// Принимает: рынок, биржу, тикер, сторону, порог, «сейчас». Ошибка — текст причины отказа.
pub fn side(
    market: &MarketState,
    venue: &VenueId,
    ticker: &str,
    buy: bool,
    limit: &Result<u64, String>,
    now: DateTime<Utc>,
) -> Result<(), String> {
    check(market, venue, limit, now)?;
    let p = market
        .pair(venue, ticker)
        .ok_or_else(|| format!("{}: пары {ticker} нет в справочнике пар", venue.0))?;
    if !p.online {
        return Err(format!(
            "{}: пара {ticker} не торгуется (status {})",
            venue.0, p.status
        ));
    }
    match (buy, p.api_buy, p.api_sell) {
        (true, false, _) => Err(format!(
            "{}: покупка {ticker} через API закрыта (apiStateBuy)",
            venue.0
        )),
        (false, _, false) => Err(format!(
            "{}: продажа {ticker} через API закрыта (apiStateSell)",
            venue.0
        )),
        _ => Ok(()),
    }
}
