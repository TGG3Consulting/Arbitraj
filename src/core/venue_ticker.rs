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

/// Ключ баланса монеты у площадки (контракт Venue::balance): у DEX — адрес контракта «0x…» (фаза 2b: DEX ищет монету по
/// адресу; монета без адреса — её тикер), у биржи — её тикер монеты. Ошибок нет.
pub fn balance_key(core: &Core, asset: &Asset, venue: &VenueId) -> String {
    let dex = rd(&core.inner.kinds)
        .iter()
        .any(|(v, k)| v == venue && *k == crate::gateway::contract::VenueKind::Dex);
    match asset.bsc_contract {
        Some(a) if dex => format!("{a:#x}"),
        _ => ticker_on(core, asset, venue),
    }
}

/// Монета из события площадки (баланс, перевод) — в монету ядра: тикер площадки известен справочнику и адрес не
/// противоречит — монета ядра (адрес, показное имя); иначе как есть (USDT, BNB, монета вне справочника). Ошибок нет.
pub fn asset_back(core: &Core, venue: &VenueId, asset: Asset) -> Asset {
    match asset_by_venue_ticker(core, venue, &asset.ticker) {
        Some(c) if asset.bsc_contract.is_none() || asset.bsc_contract == c.bsc_contract => c,
        _ => asset,
    }
}

/// Имя в ответе площадки (затрата «в монете», fee_asset, баланс) — в имя ядра: тикер этой монеты на площадке →
/// показное имя монеты ядра; иное (USDT, BNB, другие монеты) — как есть. Принимает: монету ядра, площадку, имя.
/// Ошибок нет.
pub fn name_back(core: &Core, asset: &Asset, venue: &VenueId, name: &str) -> String {
    let on = ticker_on(core, asset, venue);
    if name == on {
        asset.ticker.clone()
    } else {
        name.to_string()
    }
}

/// Затраты площадки по монете — имена в имена ядра (name_back). Ошибок нет.
pub fn costs_back(
    core: &Core,
    asset: &Asset,
    venue: &VenueId,
    costs: Vec<crate::core::types::Cost>,
) -> Vec<crate::core::types::Cost> {
    rename_costs(costs, &ticker_on(core, asset, venue), &asset.ticker)
}

/// Затраты с именем `from` (тикер площадки) — на имя `to` (имя ядра); иные — как есть. Чистая функция (DirCtx уже
/// знает тикер площадки в Presence::ticker — ядро не нужно). Ошибок нет.
pub fn rename_costs(
    costs: Vec<crate::core::types::Cost>,
    from: &str,
    to: &str,
) -> Vec<crate::core::types::Cost> {
    costs
        .into_iter()
        .map(|mut c| {
            if c.asset == from {
                c.asset = to.to_string();
            }
            c
        })
        .collect()
}

/// Котировка площадки — в монету ядра: Quote.asset — монета ядра, затраты в монете — имя ядра. Ошибок нет.
pub fn quote_back(
    core: &Core,
    asset: &Asset,
    venue: &VenueId,
    mut q: crate::core::types::Quote,
) -> crate::core::types::Quote {
    q.costs = costs_back(core, asset, venue, std::mem::take(&mut q.costs));
    q.asset = asset.clone();
    q
}

impl super::dirctx::DirCtx {
    /// Решение 211: монета для вызова площадки A — её тикер (Presence::ticker), адрес монеты ядра. Ошибок нет.
    pub fn on_a(&self) -> Asset {
        Asset {
            ticker: self.a.ticker.clone(),
            bsc_contract: self.dir.asset.bsc_contract,
        }
    }

    /// Решение 211: монета для вызова площадки B (как on_a). Ошибок нет.
    pub fn on_b(&self) -> Asset {
        Asset {
            ticker: self.b.ticker.clone(),
            bsc_contract: self.dir.asset.bsc_contract,
        }
    }
}
