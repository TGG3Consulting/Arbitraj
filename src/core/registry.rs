//! Файл: core/registry.rs
//! Что делает: справочник монет и направлений (шаг 1 ТЗ) — первичный фильтр с видимой причиной отсева:
//! монета есть на споте биржи (пара торгуется по статусу и через API, пришёл стакан); биржа выводит и принимает её
//! по сети BSC (адрес контракта сообщён); у монеты есть
//! пул с USDT на DEX; адрес контракта у биржи совпадает с адресом токена в пуле (монета узнаётся
//! по адресу контракта, а не по тикеру). Направление A→B есть, только если на A можно купить через API и вывести,
//! а на B — принять и продать через API (каждая сторона — независимо).
//! Кто вызывает: core/engine.rs (пересобирает после событий), core/coins_view.rs (экран «Монеты»).
//! От чего зависит: core/types, market_state.rs, blacklist.rs, gateway/contract (вид площадки).
//! Настройки: нет своих (сеть BSC и котируемая монета уже приведены адаптером и ядром).

use std::collections::{BTreeMap, HashMap};

use alloy_primitives::Address;
use serde::Serialize;

use crate::core::types::{Asset, CoinBlock, CoinNetworkInfo, Direction, Pool, VenueId};
use crate::gateway::contract::VenueKind;

use super::market_state::MarketState;

/// Почему монета не участвует на площадке (или в какой роли).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Exclusion {
    /// У монеты на бирже нет сети BSC.
    NoBscNetwork,
    /// Сеть BSC есть, но биржа не сообщила адрес контракта.
    NoContract,
    /// Стакан монеты не пришёл: пары нет в справочнике пар биржи (или биржа ещё не прислала справочник),
    /// или подписка ещё не ответила.
    NoSpotBook,
    /// Пара есть в справочнике биржи, но не торгуется по статусу — код статуса так, как его отдала биржа.
    PairNotTrading(i64),
    /// Справочнику пар биржи верить нельзя — текст причины: устарел (обновление не проходит дольше
    /// venue.symbols_stale_s), порог не задан или справочник не получен (pairs_fresh.rs). По паре не торгуем.
    PairsStale(String),
    /// Пара торгуется, но через API нельзя ни купить, ни продать (у BingX — apiStateBuy и apiStateSell = false):
    /// ордер через API не пройдёт.
    NoApiTrading,
    /// Не известен адрес USDT в BSC (ни одна биржа не сообщила его) — пулы искать не с чем.
    NoUsdtAddress,
    /// Пула с USDT для этого адреса контракта на DEX нет.
    NoUsdtPool,
    /// Пулы есть, но все исключены (крючки, проверки) — причины по пулам.
    PoolsExcluded(Vec<String>),
    /// Пула с USDT нет, а промежуточные монеты (решение 52) не заданы как надо — причина.
    IntermediateNotConfigured(String),
}

/// Монета на одной площадке.
#[derive(Debug, Clone, Serialize)]
pub struct Presence {
    pub venue: VenueId,
    /// Название сети BSC у биржи (для вывода и депозита); у DEX — None.
    pub network: Option<String>,
    /// Можно продавать сюда (депозит открыт; у DEX — всегда).
    pub can_receive: bool,
    /// Можно выводить отсюда (вывод открыт; у DEX — всегда).
    pub can_send: bool,
    /// Можно купить здесь через API (у биржи — пара разрешает покупку через API; у DEX — всегда).
    pub can_buy: bool,
    /// Можно продать здесь через API (у биржи — пара разрешает продажу через API; у DEX — всегда).
    pub can_sell: bool,
    /// Пусто — монета на площадке есть.
    pub excluded: Vec<Exclusion>,
    /// Данные биржи о сети BSC (комиссия, минимумы, подтверждения); у DEX — None.
    pub info: Option<CoinNetworkInfo>,
    /// DEX: монета допущена только через промежуточную монету (адрес) — пула с USDT нет (решение 52).
    pub via: Option<Address>,
}

/// Монета справочника.
#[derive(Debug, Clone, Serialize)]
pub struct CoinEntry {
    pub asset: Asset,
    pub presence: Vec<Presence>,
    /// Запись чёрного списка (заполняет engine из blacklist.rs).
    pub block: Option<CoinBlock>,
    /// Почему монету нельзя оценить (например, не заданы пороги налогов).
    pub problem: Option<String>,
}

impl CoinEntry {
    /// Площадки, где монета есть.
    pub fn venues(&self) -> Vec<VenueId> {
        self.presence
            .iter()
            .filter(|p| p.excluded.is_empty())
            .map(|p| p.venue.clone())
            .collect()
    }

    /// Рабочие направления: на A можно купить и вывести, на B — принять и продать, монета не в чёрном списке. Оба направления
    /// каждой пары считаются независимо (решение 5). Пусто — у монеты нет направлений.
    pub fn directions(&self) -> Vec<Direction> {
        if self.block.is_some() || self.problem.is_some() {
            return vec![];
        }
        let ok: Vec<&Presence> = self
            .presence
            .iter()
            .filter(|p| p.excluded.is_empty())
            .collect();
        let mut out = vec![];
        for a in ok.iter().filter(|p| p.can_send && p.can_buy) {
            for b in ok
                .iter()
                .filter(|p| p.can_receive && p.can_sell && p.venue != a.venue)
            {
                out.push(Direction {
                    asset: self.asset.clone(),
                    buy_on: a.venue.clone(),
                    sell_on: b.venue.clone(),
                });
            }
        }
        out
    }

    /// Данные о монете на площадке (если она там есть).
    pub fn at(&self, venue: &VenueId) -> Option<&Presence> {
        self.presence
            .iter()
            .find(|p| &p.venue == venue && p.excluded.is_empty())
    }
}

/// Входные данные сборки справочника.
pub struct Inputs<'a> {
    /// Площадки из реестра шлюза (только включённые) и их вид.
    pub venues: &'a [(VenueId, VenueKind)],
    /// Справочник сетей каждой биржи (coin_networks).
    pub networks: &'a HashMap<VenueId, Vec<CoinNetworkInfo>>,
    /// Адрес USDT в BSC (см. usdt_address).
    pub usdt: Option<Address>,
    /// Котируемая монета каждой биржи (venue.quote_asset) — сама в справочник не входит.
    pub quote: &'a HashMap<VenueId, String>,
    /// Название сети BSC у каждой биржи (venue.bsc_network): отличает «нет сети BSC» от «сеть BSC есть,
    /// адреса контракта нет» (presence.rs). Нет биржи в списке — различить нельзя, причина «нет сети BSC».
    pub bsc_network: &'a HashMap<VenueId, String>,
    /// Свежесть справочника пар каждой биржи (pairs_fresh::check): Err — текст причины. Нет биржи — не проверено,
    /// по её парам не торгуем.
    pub pairs_fresh: &'a HashMap<VenueId, Result<(), String>>,
    pub market: &'a MarketState,
    /// Допуск пула (blacklist::pool_admission с настройками).
    pub pool_ok: &'a dyn Fn(&Pool) -> Result<(), String>,
    /// Промежуточные монеты по каждой площадке DEX (intermediate.rs, решение 52).
    pub intermediate: &'a HashMap<VenueId, super::intermediate::Intermediate>,
}

/// Адрес USDT в BSC: из справочника бирж (котируемая монета в сети BSC).
/// Возвращает: адрес; Err — ни одна биржа его не дала или биржи расходятся (тогда DEX не торгуется).
pub fn usdt_address(
    networks: &HashMap<VenueId, Vec<CoinNetworkInfo>>,
    quote: &HashMap<VenueId, String>,
) -> Result<Address, String> {
    let mut found: Vec<(VenueId, Address)> = vec![];
    for (v, list) in networks {
        let Some(q) = quote.get(v) else { continue };
        for i in list.iter().filter(|i| &i.asset.ticker == q) {
            if let Some(a) = i.asset.bsc_contract {
                found.push((v.clone(), a));
            }
        }
    }
    match found.first() {
        None => Err("ни одна биржа не сообщила адрес котируемой монеты (USDT) в сети BSC".into()),
        Some((_, a)) if found.iter().all(|(_, b)| b == a) => Ok(*a),
        Some(_) => Err(format!("биржи дают разные адреса USDT в BSC: {found:?}")),
    }
}

/// Собирает справочник. Монеты бирж группируются по адресу контракта в BSC; монеты без адреса
/// остаются в справочнике с причиной (их видно на экране «Монеты»). Ошибок нет.
pub fn build(inp: &Inputs) -> Vec<CoinEntry> {
    // ключ: адрес контракта или тикер (у монеты без адреса) → (монета, площадка → данные сети)
    let mut coins: BTreeMap<String, (Asset, Vec<Presence>)> = BTreeMap::new();
    for (venue, _) in inp.venues.iter().filter(|(_, k)| *k == VenueKind::Cex) {
        let Some(list) = inp.networks.get(venue) else {
            continue;
        };
        let quote = inp.quote.get(venue);
        let mut by_ticker: BTreeMap<&str, Vec<&CoinNetworkInfo>> = BTreeMap::new();
        for i in list.iter().filter(|i| Some(&i.asset.ticker) != quote) {
            by_ticker
                .entry(i.asset.ticker.as_str())
                .or_default()
                .push(i);
        }
        for (ticker, nets) in by_ticker {
            let bsc = inp.bsc_network.get(venue).map(String::as_str);
            let fresh = inp.pairs_fresh.get(venue).cloned().unwrap_or_else(|| {
                Err(format!(
                    "{}: свежесть справочника пар не проверена",
                    venue.0
                ))
            });
            let p = super::presence::cex(venue, ticker, &nets, bsc, &fresh, inp.market);
            let asset = Asset {
                ticker: ticker.to_string(),
                bsc_contract: p.info.as_ref().and_then(|i| i.asset.bsc_contract),
            };
            let key = asset
                .bsc_contract
                .map(|a| a.to_string())
                .unwrap_or_else(|| format!("{}@{}", ticker, venue.0));
            coins.entry(key).or_insert((asset, vec![])).1.push(p);
        }
    }
    let dex: Vec<&VenueId> = inp
        .venues
        .iter()
        .filter(|(_, k)| *k == VenueKind::Dex)
        .map(|(v, _)| v)
        .collect();
    coins
        .into_values()
        .map(|(asset, mut presence)| {
            if let Some(token) = asset.bsc_contract {
                for v in &dex {
                    presence.push(super::presence::dex(v, token, inp));
                }
            }
            CoinEntry {
                asset,
                presence,
                block: None,
                problem: None,
            }
        })
        .collect()
}
