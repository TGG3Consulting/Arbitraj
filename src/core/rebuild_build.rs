//! Файл: core/rebuild_build.rs
//! Что делает: сборка части справочника монет (рабочие или неподвижные — решение 131, coin_book.rs) на снимках рынка и
//! сетей, без блокировок и без await (решение 129): свежесть справочника пар каждой биржи, допуск пулов, промежуточные
//! монеты DEX → registry_build::build_part; решение 430 — по сетям (registry_chains.rs: площадки DEX, записи сетей
//! бирж, USDT и нативная монета сети; монете — сеть группы).
//! Решение 489: монеты сети по умолчанию собираются только из записей без сети — даже при одной группе: записи с сетью
//! (Asset.chain = Some) в справочнике сетей, а группы их сети нет (площадку сети выключили, справочник сетей бирж ещё не
//! перечитан), в сеть по умолчанию не попадают (иначе — адрес другой сети без сети, инцидент 21.11.2026). Сеть, которую
//! держат только идущие сделки выключенной площадки (chain_play::in_play), собирается в своей группе без DEX-присутствия, а
//! её монетам ставится problem (held_only) — направлений, а значит и новых сделок, нет (executor.rs:97).
//! Кто вызывает: core/rebuild.rs.
//! От чего зависит: chain.rs (dexes), registry_chains.rs, registry_build.rs, pairs_fresh.rs, blacklist.rs, intermediate.rs, market_state.rs, settings.
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
    // Решение 489: группы — по площадкам DEX в игре (реестр: включена или её держат идущие сделки); в группе остаются
    // только включённые (DEX-присутствие, нативная монета — как до решения 489).
    let groups = super::registry_chains::groups(core, &super::chain::dexes(core));
    // Решение 489: одна группа и записей с сетью нет — входы прежние, без копий (решение 129); записи с сетью есть —
    // сеть по умолчанию берёт только свои (nets_of), как при нескольких группах.
    let tagged = nets
        .values()
        .any(|l| l.iter().any(|i| i.asset.chain.is_some()));
    let single = groups.len() == 1 && !tagged;
    let mut out = Vec::new();
    for mut g in groups {
        let stop = held_only(&mut g, &dex);
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
                Some(_) => super::registry_build::usdt_address_in(&own, quote, g.tag.as_ref()).ok(),
            };
            (&own, u)
        };
        // Решение 431: имя сети биржи для своей группы — presence.rs различает «нет сети» и «сеть есть, адреса нет» по
        // имени сети ЭТОЙ группы (сеть по умолчанию — venue.bsc_network, как раньше; другая — venue.<сеть>_network).
        let names_own;
        let names_g = match &g.tag {
            None => bsc_network,
            Some(c) => {
                names_own = other_names(s, venues, c);
                &names_own
            }
        };
        // Ревью К1 (решение 211): нативная монета и обёртка — общая настройка площадок DEX сети (native.rs).
        let native = super::native::read(s, &g.dex).ok().flatten();
        let mut part_g = super::registry_build::build_part(
            &Inputs {
                venues: &gv,
                networks: nets_g,
                usdt: usdt_g,
                quote,
                bsc_network: names_g,
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
                // Решение 489: сеть держат только идущие сделки — монета есть (сделки ведутся, перегон находит её),
                // направлений нет (CoinEntry::directions при problem пусто; присутствия не трогаются).
                if let Some(why) = &stop {
                    c.problem = Some(why.clone());
                }
            }
        }
        out.extend(part_g);
    }
    out
}

/// Что делает: решение 489 — оставляет в группе только включённые площадки DEX (`on`); сеть не по умолчанию, у которой
/// включённых не осталось (её держат только идущие сделки, решение 196), — причина «новых сделок нет». Сеть по умолчанию
/// не останавливается (монеты бирж без DEX — как до решения 489). Принимает: группу, включённые площадки DEX.
/// Возвращает: причину или None. Ошибок нет.
fn held_only(g: &mut super::registry_chains::Group, on: &[VenueId]) -> Option<String> {
    let off: Vec<String> = g
        .dex
        .iter()
        .filter(|v| !on.contains(v))
        .map(|v| v.0.clone())
        .collect();
    g.dex.retain(|v| on.contains(v));
    let c = g.tag.as_ref()?;
    (g.dex.is_empty() && !off.is_empty()).then(|| {
        format!(
            "сеть {c}: площадка {} выключена — только идущие сделки (решение 196)",
            off.join(", ")
        )
    })
}

/// Что делает: имя сети `c` у каждой биржи (venue.<c>_network, gateway/own_nets.rs; решение 431). Принимает: хранилище,
/// площадки, сеть. Возвращает: биржа → имя (не задано — биржи нет в карте). Ошибок нет.
fn other_names(
    s: &crate::settings::store::Store,
    venues: &[(VenueId, VenueKind)],
    c: &crate::core::types::ChainId,
) -> HashMap<VenueId, String> {
    venues
        .iter()
        .filter(|(_, k)| *k == VenueKind::Cex)
        .filter_map(|(v, _)| {
            let at = crate::settings::read::At {
                venue: Some(&v.0),
                coin: None,
            };
            crate::gateway::own_nets::name_of(s, at, Some(c))
                .ok()
                .map(|n| (v.clone(), n))
        })
        .collect()
}
