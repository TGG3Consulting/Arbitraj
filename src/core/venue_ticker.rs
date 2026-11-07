//! Файл: core/venue_ticker.rs
//! Что делает: решение 211 (Тигран, 07.11.2026: «метчить монеты только по адресам … сквозную»): ЕДИНСТВЕННЫЙ
//! переводчик между монетой ядра (адрес контракта BSC + показное имя) и монетой площадки (тот же адрес + тикер этой
//! площадки). Биржи знают монету только по своему тикеру (WS-стакан по символу пары, ордер, вывод, адрес депозита,
//! статусы, балансы, события аккаунта — исследование A 07.11); адрес приходит лишь из их справочника сетей. Связь
//! «тикер площадки ↔ адрес» строит справочник (registry_build.rs → Presence::ticker), карты держит ticker_index.rs.
//! В шлюз монета уходит через on_venue (тикер площадки, адрес тот же); всё, что приходит от площадки с её тикером
//! (стакан, пара, баланс, перевод, делистинг, затраты в монете), переводится в монету ядра через asset_by_venue_ticker.
//! Монета без присутствия на площадке (USDT, BNB газа, монеты venue.price_assets, монета вне справочника) — как есть.
//! Кто вызывает: core (subscribe, tick и все вызовы шлюза с монетой — фаза 2a решения 211).
//! От чего зависит: ticker_index.rs (карты), engine.rs.
//! Настройки: нет.

use crate::core::types::{Asset, VenueId};

use super::engine::{Core, rd};
use super::ticker_index::TickerIndex;

impl TickerIndex {
    /// Тикер монеты на площадке. Принимает: монету (ядра или площадки), площадку. Возвращает: тикер площадки; монета
    /// без адреса или без присутствия на площадке — её собственный тикер. Ошибок нет.
    pub fn ticker_on(&self, asset: &Asset, venue: &VenueId) -> String {
        asset
            .bsc_contract
            .and_then(|a| self.venue_ticker.get(&(venue.clone(), a)).cloned())
            .unwrap_or_else(|| asset.ticker.clone())
    }

    /// Монета для вызова площадки: тот же адрес, тикер площадки. Ошибок нет.
    pub fn on_venue(&self, asset: &Asset, venue: &VenueId) -> Asset {
        Asset {
            ticker: self.ticker_on(asset, venue),
            bsc_contract: asset.bsc_contract,
        }
    }
}

/// Монета для вызова площадки (тикер площадки, адрес тот же). Ошибок нет.
pub fn on_venue(core: &Core, asset: &Asset, venue: &VenueId) -> Asset {
    rd(&core.inner.tickers).on_venue(asset, venue)
}

/// Тикер монеты на площадке — для стакана, пары и балансов площадки (MarketState хранит их по тикеру площадки).
/// Ошибок нет.
pub fn ticker_on(core: &Core, asset: &Asset, venue: &VenueId) -> String {
    rd(&core.inner.tickers).ticker_on(asset, venue)
}

/// Монета ядра (адрес, показное имя) по тикеру площадки. None — такой монеты площадки в справочнике нет. Ошибок нет.
pub fn asset_by_venue_ticker(core: &Core, venue: &VenueId, ticker: &str) -> Option<Asset> {
    rd(&core.inner.tickers).get(venue, ticker)
}
