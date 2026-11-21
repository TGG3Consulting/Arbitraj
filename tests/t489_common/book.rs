//! Файл: tests/t489_common/book.rs
//! Что делает: решение 489 — справочник монет для проверок, собранный напрямую (rebuild_build::build — та же сборка, что
//! у core/rebuild.rs, без фоновой задачи пересборки: тест детерминирован), USDT сетей не по умолчанию (как rebuild.rs
//! usdt_other), наборы подписки DEX и биржи по тем же условиям, что core/subscribe.rs wanted, монеты сети с адресами из
//! набора.
//! Кто вызывает: tests/core_net_chain_489.rs, tests/core_net_chain_489_held.rs, tests/core_net_chain_489_recap.rs.
//! От чего зависит: arb::core (chain, rebuild_build, registry_build, registry_chains, registry_sync, subscribe).
//! Моков нет.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use alloy_primitives::Address;

use arb::core::Core;
use arb::core::rebuild_build::build;
use arb::core::registry::CoinEntry;
use arb::core::registry_build::{Networks, Part, usdt_address};
use arb::core::types::{ChainId, VenueId};
use arb::gateway::contract::VenueKind;

/// Входы сборки: площадки, котируемые монеты, справочник сетей.
type In = (
    Vec<(VenueId, VenueKind)>,
    HashMap<VenueId, String>,
    Networks,
);

/// Входы сборки, как у core/rebuild.rs: включённые площадки, котируемая монета и имя сети BSC биржи bingx, справочник
/// сетей. Ошибок нет.
fn inputs(core: &Core) -> In {
    let kinds = core.inner.kinds.read().unwrap().clone();
    let mut vs: Vec<(VenueId, VenueKind)> = vec![];
    for (x, k) in kinds {
        if arb::core::registry_sync::enabled(core, &x) {
            vs.push((x, k));
        }
    }
    let q = HashMap::from([(VenueId("bingx".into()), "USDT".to_string())]);
    let n: Networks = core.inner.networks.read().unwrap().clone();
    (vs, q, n)
}

/// Справочник монет ядра (рабочие и неподвижные) по текущим входам. Ошибок нет.
pub fn built(core: &Core) -> Vec<CoinEntry> {
    let (vs, q, n) = inputs(core);
    let b = HashMap::from([(VenueId("bingx".into()), "BEP20".to_string())]);
    let m = core.inner.market.read().unwrap().registry_view();
    let u = usdt_address(&n, &q).ok();
    let mut out = build(core, &vs, &q, &b, u, &n, &m, Part::Live);
    out.extend(build(core, &vs, &q, &b, u, &n, &m, Part::Inert));
    out
}

/// USDT сетей не по умолчанию — как rebuild.rs: группы по реестру (registry_chains::groups) и usdt_other. Ошибок нет.
pub fn usdt_other(core: &Core) -> HashMap<ChainId, Address> {
    let (_, q, n) = inputs(core);
    let dex = arb::core::chain::dexes(core);
    let groups = arb::core::registry_chains::groups(core, &dex);
    arb::core::registry_chains::usdt_other(&groups, &n, &q)
}

/// Набор подписки площадки DEX — условие ОБЯЗАНО совпадать с core/subscribe.rs wanted, ветка VenueKind::Dex
/// (subscribe.rs:51-57: монеты сети площадки — chain::tag_of_dex — и subscribe::dex_tradable); сама wanted приватная.
/// Принимает: ядро, справочник, площадку. Паника — сеть площадки не прочитана.
pub fn dex_set(core: &Core, list: &[CoinEntry], dex: &str) -> Vec<CoinEntry> {
    let venue = VenueId(dex.into());
    let tag = arb::core::chain::tag_of_dex(core, &venue).unwrap();
    let kinds = core.inner.kinds.read().unwrap().clone();
    let market = core.inner.market.read().unwrap();
    let mut out = vec![];
    for c in list {
        let tradable = arb::core::subscribe::dex_tradable(c, &kinds, &market);
        if c.asset.chain == tag && tradable {
            out.push(c.clone());
        }
    }
    out
}

/// Монеты в подписке биржи — условие ОБЯЗАНО совпадать с core/subscribe.rs wanted, ветка VenueKind::Cex
/// (subscribe.rs:32-45: у монеты присутствие этой биржи с данными сети — Presence::info). Ошибок нет.
pub fn cex_set(list: &[CoinEntry], cex: &str) -> Vec<CoinEntry> {
    let mut out = vec![];
    for c in list {
        if c.presence
            .iter()
            .any(|p| p.venue.0 == cex && p.info.is_some())
        {
            out.push(c.clone());
        }
    }
    out
}

/// Монеты списка в сети `tag` (None — без сети) с адресом из `only`. Ошибок нет.
pub fn pick(list: &[CoinEntry], tag: Option<&ChainId>, only: &HashSet<Address>) -> Vec<CoinEntry> {
    let mut out = vec![];
    for c in list {
        let hit = c.asset.contract.is_some_and(|a| only.contains(&a));
        if c.asset.chain.as_ref() == tag && hit {
            out.push(c.clone());
        }
    }
    out
}

/// Сколько монет списка с сетью (Asset.chain = Some). Ошибок нет.
pub fn chained(list: &[CoinEntry]) -> usize {
    list.iter().filter(|c| c.asset.chain.is_some()).count()
}
