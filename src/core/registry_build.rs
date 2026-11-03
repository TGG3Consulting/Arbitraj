//! Файл: core/registry_build.rs
//! Что делает: сборка справочника монет (core/registry.rs) из сетей бирж, рынка и пулов DEX: присутствие на каждой
//! бирже и — только для монет подписки DEX (core/subscribe.rs cex_pair) — на каждой DEX; адрес USDT в BSC по справочникам бирж.
//! Кто вызывает: core/rebuild.rs, tests/core_dex_subscription.rs.
//! От чего зависит: registry.rs (CoinEntry, Presence), presence.rs, market_state.rs, intermediate.rs, gateway/contract.
//! Настройки: нет своих.

use std::collections::{BTreeMap, HashMap};

use alloy_primitives::Address;

use crate::core::types::{Asset, CoinNetworkInfo, Pool, VenueId};
use crate::gateway::contract::VenueKind;

use super::market_state::MarketState;
use super::registry::{CoinEntry, Presence};

/// Справочник сетей бирж: биржа → её сети. Списки за `Arc` (решение 129): снимок для пересборки справочника монет
/// — копия указателей, без копии тысяч записей MEXC и без удержания блокировки сетей во время сборки.
pub type Networks = HashMap<VenueId, std::sync::Arc<Vec<CoinNetworkInfo>>>;

/// Входные данные сборки справочника.
pub struct Inputs<'a> {
    /// Площадки из реестра шлюза (только включённые) и их вид.
    pub venues: &'a [(VenueId, VenueKind)],
    /// Справочник сетей каждой биржи (coin_networks).
    pub networks: &'a Networks,
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
    networks: &Networks,
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

/// Собирает справочник. Монеты бирж группируются по адресу контракта в BSC и тикеру (решение 118); монеты без адреса
/// остаются в справочнике с причиной (их видно на экране «Монеты»). Ошибок нет.
pub fn build(inp: &Inputs) -> Vec<CoinEntry> {
    build_part(inp, Part::All)
}

/// Какую часть справочника собирать (решение 131, coin_book.rs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// Все монеты.
    All,
    /// Рабочие: хотя бы у одной сети монеты на бирже есть адрес контракта.
    Live,
    /// Неподвижные: адреса контракта нет ни в одной сети монеты на бирже.
    Inert,
}

/// Собирает часть справочника (как build, только монеты этой части; у неподвижных монет запись — по бирже, адреса
/// нет, поэтому части не пересекаются и вместе дают то же, что build). Ошибок нет.
pub fn build_part(inp: &Inputs, part: Part) -> Vec<CoinEntry> {
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
            let live = nets.iter().any(|i| i.asset.bsc_contract.is_some());
            if (part == Part::Live && !live) || (part == Part::Inert && live) {
                continue;
            }
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
            // Решение 118 (Я-8): ключ — адрес и тикер. У одного адреса на разных биржах разные тикеры — отдельные
            // записи (направлений между ними нет: бот не торгует то, что не может сопоставить; журнал —
            // ticker_index.rs). Одинаковый тикер — одна запись, как раньше.
            let key = asset
                .bsc_contract
                .map(|a| format!("{a}|{ticker}"))
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
            // DEX ищет пулы только монет своей подписки (core/subscribe.rs: то же условие, cex_pair) — у остальных
            // DEX-присутствия нет: «нет пула с USDT» им не ставится (решение 61).
            let searched =
                super::subscribe::cex_pair(&presence, &asset.ticker, inp.venues, inp.market);
            if let Some(token) = asset.bsc_contract.filter(|_| searched) {
                for v in &dex {
                    presence.push(super::presence::dex(v, token, inp));
                }
            }
            CoinEntry {
                asset,
                presence,
                block: None,
                problem: None,
                wait: None,
                cex_cex: None,
            }
        })
        .collect()
}
