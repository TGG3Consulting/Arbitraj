//! Файл: core/ticker_index.rs
//! Что делает: карта (площадка, тикер) → монета справочника для тика стакана (решение 118, Я-9): событие стакана
//! находит монету своей площадки за O(1), без перебора всего справочника (с MEXC — ~11 тыс. записей) и без чужого
//! токена с тем же тикером на другой бирже. Плюс журнал Я-8: у одного адреса контракта на разных биржах разные
//! тикеры — справочник делит монету на записи по тикеру (registry_build.rs), направлений между ними нет; запись в
//! журнал — одна на изменение. Решение 129: ещё адрес → монеты (by_tokens) для события пула.
//! Решение 200: расхождения тикеров отдаются интерфейсу (ticker_splits → GET /api/coins/ticker-splits) — у каждого
//! кнопка «Считать одной монетой» (склейка coin.ticker_alias делается в шлюзе, gateway/aliased.rs).
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
    /// (площадка, тикер) → монета (у монеты есть адрес контракта; монеты без адреса не торгуются и не ищутся).
    pub map: HashMap<(VenueId, String), Asset>,
    /// Адрес контракта → монеты справочника с этим адресом (решение 129: событие пула находит монеты своих токенов
    /// без прохода всего справочника — у MEXC тысячи монет на каждое событие пула).
    pub by_token: HashMap<Address, Vec<Asset>>,
    /// Монета → её место в справочнике (inner.coins) на момент сборки (решение 131: пересчёт монеты на тике находит
    /// запись без прохода справочника; место сверяется с монетой — справочник мог смениться).
    pub pos: HashMap<Asset, usize>,
    /// Адрес → текст уже записанного расхождения тикеров (чтобы не повторять).
    told: HashMap<Address, String>,
    /// Решение 200: текущие расхождения тикеров по адресу — (биржа, тикер) — для интерфейса.
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

/// Пересобирает карту по справочнику и пишет в журнал новые расхождения тикеров одного адреса (решение 118, Я-8).
/// Принимает: ядро, справочник. Ошибок нет.
pub async fn rebuild(core: &Core, coins: &[CoinEntry]) {
    let mut map = HashMap::new();
    let mut by_token: HashMap<Address, Vec<Asset>> = HashMap::new();
    let pos: HashMap<Asset, usize> = coins
        .iter()
        .enumerate()
        .map(|(i, c)| (c.asset.clone(), i))
        .collect();
    for c in coins {
        if let Some(addr) = c.asset.bsc_contract {
            by_token.entry(addr).or_default().push(c.asset.clone());
        }
    }
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
    let splits: BTreeMap<Address, BTreeSet<(String, String)>> = by_addr
        .into_iter()
        .filter(|(_, v)| {
            let first = v.iter().next().map(|(_, t)| t);
            v.iter().any(|(_, t)| Some(t) != first)
        })
        .collect();
    let conflicts: HashMap<Address, String> = splits
        .iter()
        .map(|(a, v)| {
            let list: Vec<String> = v.iter().map(|(venue, t)| format!("{venue}={t}")).collect();
            (*a, list.join(", "))
        })
        .collect();
    let fresh: Vec<(Address, String)> = {
        let mut idx = wr(&core.inner.tickers);
        idx.map = map;
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
            "У адреса {a} разные тикеры: {t} — монета разделена по тикеру, направлений между ними нет (склеить — «Монеты» → «Считать одной монетой», coin.ticker_alias)"
        );
        core.inner
            .deps
            .log(Level::Warn, "core.ticker_split", None, text, None)
            .await;
    }
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
