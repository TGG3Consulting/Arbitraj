//! Файл: tests/t489_common/book.rs
//! Что делает: решение 489 — справочник монет ядра для проверок: все монеты, набор подписки площадки DEX (то же
//! условие, что core/subscribe.rs wanted), монеты сети с адресами из набора.
//! Кто вызывает: tests/t489_common/mod.rs (переэкспорт), тесты core_net_chain_489*.rs.
//! От чего зависит: arb::core (chain, subscribe, registry). Моков нет.

#![allow(dead_code)]

use std::collections::HashSet;

use alloy_primitives::Address;

use arb::core::Core;
use arb::core::registry::CoinEntry;
use arb::core::types::{ChainId, VenueId};

/// Все монеты справочника ядра (рабочие и неподвижные). Ошибок нет.
pub fn coins(core: &Core) -> Vec<CoinEntry> {
    let book = core.coins();
    let mut out = vec![];
    for c in book.all() {
        out.push(c.clone());
    }
    out
}

/// Набор подписки площадки DEX по справочнику — то же условие, что core/subscribe.rs wanted (монеты сети площадки,
/// chain::tag_of_dex, и subscribe::dex_tradable). Паника — сеть площадки не прочитана.
pub fn dex_set(core: &Core, dex: &str) -> Vec<CoinEntry> {
    let venue = VenueId(dex.into());
    let tag = arb::core::chain::tag_of_dex(core, &venue).unwrap();
    let kinds = core.inner.kinds.read().unwrap().clone();
    let market = core.inner.market.read().unwrap();
    let mut out = vec![];
    for c in coins(core) {
        let tradable = arb::core::subscribe::dex_tradable(&c, &kinds, &market);
        if c.asset.chain == tag && tradable {
            out.push(c);
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
