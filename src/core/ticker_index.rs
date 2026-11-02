//! Файл: core/ticker_index.rs
//! Что делает: карта (площадка, тикер) → монета справочника для тика стакана (решение 118, Я-9): событие стакана
//! находит монету своей площадки за O(1), без перебора всего справочника (с MEXC — ~11 тыс. записей) и без чужого
//! токена с тем же тикером на другой бирже. Плюс журнал Я-8: у одного адреса контракта на разных биржах разные
//! тикеры — справочник делит монету на записи по тикеру (registry_build.rs), направлений между ними нет; запись в
//! журнал — одна на изменение.
//! Кто вызывает: core/rebuild.rs (после сборки справочника), core/tick.rs (поиск монеты по событию).
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
    /// (площадка, тикер) → монета (у монеты есть адрес контракта; монеты без адреса не торгуются и не ищутся).
    pub map: HashMap<(VenueId, String), Asset>,
    /// Адрес → текст уже записанного расхождения тикеров (чтобы не повторять).
    told: HashMap<Address, String>,
}

impl TickerIndex {
    /// Монета площадки по тикеру. None — нет в справочнике. Ошибок нет.
    pub fn get(&self, venue: &VenueId, ticker: &str) -> Option<Asset> {
        self.map.get(&(venue.clone(), ticker.to_string())).cloned()
    }
}

/// Пересобирает карту по справочнику и пишет в журнал новые расхождения тикеров одного адреса (решение 118, Я-8).
/// Принимает: ядро, справочник. Ошибок нет.
pub async fn rebuild(core: &Core, coins: &[CoinEntry]) {
    let mut map = HashMap::new();
    // адрес → (биржа, тикер)
    let mut by_addr: BTreeMap<Address, BTreeSet<(String, String)>> = BTreeMap::new();
    for c in coins {
        let Some(addr) = c.asset.bsc_contract else {
            continue;
        };
        for p in &c.presence {
            // Тикер у DEX — тот, что передан в подписку (тикер записи); при совпадении — первая запись.
            map.entry((p.venue.clone(), c.asset.ticker.clone()))
                .or_insert_with(|| c.asset.clone());
            if p.info.is_some() {
                by_addr
                    .entry(addr)
                    .or_default()
                    .insert((p.venue.0.clone(), c.asset.ticker.clone()));
            }
        }
    }
    let conflicts: HashMap<Address, String> = by_addr
        .into_iter()
        .filter(|(_, v)| {
            let first = v.iter().next().map(|(_, t)| t);
            v.iter().any(|(_, t)| Some(t) != first)
        })
        .map(|(a, v)| {
            let list: Vec<String> = v.iter().map(|(venue, t)| format!("{venue}={t}")).collect();
            (a, list.join(", "))
        })
        .collect();
    let fresh: Vec<(Address, String)> = {
        let mut idx = wr(&core.inner.tickers);
        idx.map = map;
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
            "У адреса {a} разные тикеры: {t} — монета разделена по тикеру, направлений между ними нет"
        );
        core.inner
            .deps
            .log(Level::Warn, "core.ticker_split", None, text, None)
            .await;
    }
}
