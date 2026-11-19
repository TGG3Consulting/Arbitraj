//! Файл: core/rebuild_build.rs
//! Что делает: сборка части справочника монет (рабочие или неподвижные — решение 131, coin_book.rs) на снимках рынка и
//! сетей, без блокировок и без await (решение 129): свежесть справочника пар каждой биржи, допуск пулов, промежуточные
//! монеты DEX → registry_build::build_part; решение 430 — по сетям (registry_chains.rs: площадки DEX, записи сетей
//! бирж, USDT и нативная монета сети; монете — сеть группы).
//! Кто вызывает: core/rebuild.rs.
//! От чего зависит: registry_chains.rs, registry_build.rs, pairs_fresh.rs, blacklist.rs, intermediate.rs, market_state.rs, settings.
//! Настройки: через вызываемые — venue.symbols_stale_s, hooks.*, dex.intermediate_*.

use std::collections::HashMap;

use chrono::Utc;

use crate::core::types::{Pool, VenueId};
use crate::gateway::contract::VenueKind;

use super::blacklist;
use super::engine::Core;
use super::market_state::MarketState;
use super::registry::{CoinEntry, Inputs};
use super::registry_build::{Networks, Part};

/// Сборка части справочника на снимках рынка и сетей (решения 129, 131: без блокировок, без await).
#[allow(clippy::too_many_arguments)]
pub fn build(
    core: &Core,
    venues: &[(VenueId, VenueKind)],
    quote: &HashMap<VenueId, String>,
    bsc_network: &HashMap<VenueId, String>,
    usdt: Option<alloy_primitives::Address>,
    nets: &Networks,
    market: &MarketState,
    part: Part,
) -> Vec<CoinEntry> {
    let s = &core.inner.deps.settings;
    // Свежесть справочника пар каждой биржи (pairs_fresh.rs): устаревший — по её парам не торгуем.
    let now = Utc::now();
    let pairs_fresh: HashMap<VenueId, Result<(), String>> = venues
        .iter()
        .filter(|(_, k)| *k == VenueKind::Cex)
        .map(|(v, _)| {
            let limit = super::pairs_fresh::limit(s, v);
            (v.clone(), super::pairs_fresh::check(market, v, &limit, now))
        })
        .collect();
    let pool_ok =
        |p: &Pool| blacklist::pool_admission(p, market.pool_check(&p.venue, &p.reference), s);
    let intermediate: HashMap<VenueId, super::intermediate::Intermediate> = venues
        .iter()
        .filter(|(_, k)| *k == VenueKind::Dex)
        .map(|(v, _)| (v.clone(), super::intermediate::read(s, v)))
        .collect();
    let dex: Vec<VenueId> = venues
        .iter()
        .filter(|(_, k)| *k == VenueKind::Dex)
        .map(|(v, _)| v.clone())
        .collect();
    // Решение 430: справочник по сетям (registry_chains.rs) — у каждой сети свои площадки DEX, записи сетей бирж, USDT и
    // нативная монета; монете ставится сеть группы. Одна группа — входы прежние (без копий справочника сетей).
    let groups = super::registry_chains::groups(core, &dex);
    let single = groups.len() == 1;
    let mut out = Vec::new();
    for g in groups {
        let gv: Vec<(VenueId, VenueKind)> = venues
            .iter()
            .filter(|(v, k)| *k == VenueKind::Cex || g.dex.contains(v))
            .cloned()
            .collect();
        let own;
        let (nets_g, usdt_g) = if single {
            (nets, usdt)
        } else {
            own = super::registry_chains::nets_of(nets, g.tag.as_ref());
            let u = match &g.tag {
                None => usdt,
                Some(_) => super::registry_build::usdt_address(&own, quote).ok(),
            };
            (&own, u)
        };
        // Ревью К1 (решение 211): нативная монета и обёртка — общая настройка площадок DEX сети (native.rs).
        let native = super::native::read(s, &g.dex).ok().flatten();
        let mut part_g = super::registry_build::build_part(
            &Inputs {
                venues: &gv,
                networks: nets_g,
                usdt: usdt_g,
                quote,
                bsc_network,
                pairs_fresh: &pairs_fresh,
                market,
                pool_ok: &pool_ok,
                intermediate: &intermediate,
                native: native.as_ref(),
            },
            part,
        );
        if g.tag.is_some() {
            for c in &mut part_g {
                c.asset.chain = g.tag.clone();
            }
        }
        out.extend(part_g);
    }
    out
}
