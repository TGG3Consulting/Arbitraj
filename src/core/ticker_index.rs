//! Файл: core/ticker_index.rs
//! Что делает: карты для тика и перевода тикеров (решение 118, Я-9; решение 211 — монета = адрес): (площадка, тикер
//! площадки) → монета справочника — событие стакана находит монету своей площадки за O(1) без чужого токена с тем
//! же тикером на другой бирже; (площадка, адрес) → тикер площадки (core/venue_ticker.rs); адрес → монеты
//! (by_tokens, решение 129) для события пула. Журнал: у адреса на ОДНОЙ бирже несколько тикеров и выбрать нельзя
//! (registry_merge.rs) — одна запись на изменение; эти конфликты отдаются интерфейсу (ticker_splits).
//! Кто вызывает: core/rebuild.rs (после сборки справочника), core/tick.rs (поиск монеты по событию), api/coins.rs.
//! От чего зависит: registry.rs (CoinEntry), core/types, deps.rs (журнал).
//! Настройки: нет.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use alloy_primitives::Address;

use crate::core::types::{Asset, VenueId};
use crate::storage::journal::Level;

use super::engine::{Core, wr};
use super::registry::CoinEntry;

/// Карта тикеров и последние сообщённые расхождения тикеров по адресу.
#[derive(Default)]
pub struct TickerIndex {
    /// (площадка, ТИКЕР ПЛОЩАДКИ) → монета ядра (решение 211; у монеты есть адрес контракта; монеты без адреса не
    /// торгуются и не ищутся).
    pub map: HashMap<(VenueId, String), Asset>,
    /// Решение 211: (площадка, адрес) → тикер этой площадки (core/venue_ticker.rs).
    pub venue_ticker: HashMap<(VenueId, Address), String>,
    /// Адрес контракта → монеты справочника с этим адресом (решение 129: событие пула находит монеты своих токенов
    /// без прохода всего справочника — у MEXC тысячи монет на каждое событие пула).
    pub by_token: HashMap<Address, Vec<Asset>>,
    /// Монета → её место в справочнике (inner.coins) на момент сборки (решение 131: пересчёт монеты на тике находит
    /// запись без прохода справочника; место сверяется с монетой — справочник мог смениться).
    pub pos: HashMap<Asset, usize>,
    /// Адрес → текст уже записанного конфликта тикеров (чтобы не повторять).
    told: HashMap<Address, String>,
    /// Решение 211: конфликты тикеров по адресу — (биржа, тикеры в конфликте) — для интерфейса.
    splits: BTreeMap<Address, BTreeSet<(String, String)>>,
}

/// Расхождение тикеров одного адреса для интерфейса (решение 200).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TickerSplit {
    pub address: Address,
    /// (биржа, тикер) по порядку.
    pub venues: Vec<(String, String)>,
}

impl Core {
    /// Текущие расхождения тикеров одного адреса (решение 200). Ошибок нет.
    pub fn ticker_splits(&self) -> Vec<TickerSplit> {
        super::engine::rd(&self.inner.tickers)
            .splits
            .iter()
            .map(|(a, v)| TickerSplit {
                address: *a,
                venues: v.iter().cloned().collect(),
            })
            .collect()
    }
}

impl TickerIndex {
    /// Монета площадки по тикеру. None — нет в справочнике. Ошибок нет.
    pub fn get(&self, venue: &VenueId, ticker: &str) -> Option<Asset> {
        self.map.get(&(venue.clone(), ticker.to_string())).cloned()
    }

    /// Монеты справочника, чей адрес контракта — один из двух токенов пула. Ошибок нет.
    pub fn by_tokens(&self, t: (Address, Address)) -> Vec<Asset> {
        let mut out: Vec<Asset> = self.by_token.get(&t.0).cloned().unwrap_or_default();
        if t.1 != t.0 {
            out.extend(self.by_token.get(&t.1).into_iter().flatten().cloned());
        }
        out
    }
}

/// Пересобирает карты по справочнику (решение 211: монета — адрес; тикер у каждой площадки свой — Presence::ticker) и
/// пишет в журнал новые конфликты тикеров (у адреса на одной бирже несколько тикеров, выбрать нельзя —
/// registry_merge.rs). Принимает: ядро, справочник. Ошибок нет.
pub async fn rebuild(core: &Core, coins: &[CoinEntry]) {
    let TickerIndex {
        map,
        venue_ticker,
        by_token,
        pos,
        splits,
        ..
    } = index_of(coins);
    let conflicts: HashMap<Address, String> = splits
        .iter()
        .map(|(a, v)| {
            let list: Vec<String> = v.iter().map(|(venue, t)| format!("{venue}: {t}")).collect();
            (*a, list.join("; "))
        })
        .collect();
    let fresh: Vec<(Address, String)> = {
        let mut idx = wr(&core.inner.tickers);
        idx.map = map;
        idx.venue_ticker = venue_ticker;
        idx.by_token = by_token;
        idx.pos = pos;
        idx.splits = splits;
        let new = conflicts
            .iter()
            .filter(|(a, t)| idx.told.get(*a) != Some(*t))
            .map(|(a, t)| (*a, t.clone()))
            .collect();
        idx.told = conflicts;
        new
    };
    for (a, t) in fresh {
        let text = format!(
            "У адреса {a} на бирже несколько тикеров, выбрать нельзя ({t}) — монета на этой бирже не торгуется (решение 211)"
        );
        core.inner
            .deps
            .log(Level::Warn, "core.ticker_conflict", None, text, None)
            .await;
    }
}

/// Карты по справочнику без журнала (решение 211) — чистая сборка: (площадка, тикер площадки) → монета, (площадка,
/// адрес) → тикер площадки, адрес → монеты, место монеты, конфликты тикеров. Принимает: справочник. Ошибок нет.
pub fn index_of(coins: &[CoinEntry]) -> TickerIndex {
    let mut idx = TickerIndex {
        pos: coins
            .iter()
            .enumerate()
            .map(|(i, c)| (c.asset.clone(), i))
            .collect(),
        ..TickerIndex::default()
    };
    for c in coins {
        let Some(addr) = c.asset.bsc_contract else {
            continue;
        };
        idx.by_token.entry(addr).or_default().push(c.asset.clone());
        for p in &c.presence {
            // Тикер ЭТОЙ площадки → монета ядра (у DEX — показное имя, его и передаёт подписка).
            idx.map
                .entry((p.venue.clone(), p.ticker.clone()))
                .or_insert_with(|| c.asset.clone());
            idx.venue_ticker
                .entry((p.venue.clone(), addr))
                .or_insert_with(|| p.ticker.clone());
            for e in &p.excluded {
                if let super::registry::Exclusion::TickerConflict(t) = e {
                    idx.splits
                        .entry(addr)
                        .or_default()
                        .insert((p.venue.0.clone(), t.clone()));
                }
            }
        }
    }
    idx
}

/// Запись монеты среди рабочих монет справочника (решение 131): по карте места, без прохода справочника; карта от
/// прежнего справочника (между записью карты и справочника) — проходом, как раньше. Принимает: ядро, рабочие монеты
/// (под блокировкой вызывающего), монету. Возвращает: запись; None — монеты нет. Ошибок нет.
pub fn find<'a>(core: &Core, coins: &'a [CoinEntry], asset: &Asset) -> Option<&'a CoinEntry> {
    let at = super::engine::rd(&core.inner.tickers)
        .pos
        .get(asset)
        .copied();
    at.and_then(|i| coins.get(i))
        .filter(|c| &c.asset == asset)
        .or_else(|| coins.iter().find(|c| &c.asset == asset))
}

/// Первая монета справочника с этим адресом контракта (порядок справочника) — по карте адрес → монеты. Ошибок нет.
pub fn find_by_token<'a>(
    core: &Core,
    coins: &'a [CoinEntry],
    token: Address,
) -> Option<&'a CoinEntry> {
    let first = super::engine::rd(&core.inner.tickers)
        .by_token
        .get(&token)
        .and_then(|v| v.first().cloned());
    match first {
        Some(a) => find(core, coins, &a),
        None => coins.iter().find(|c| c.asset.bsc_contract == Some(token)),
    }
}
