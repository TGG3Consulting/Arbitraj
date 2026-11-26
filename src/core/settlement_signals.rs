//! Файл: core/settlement_signals.rs.
//! Что делает: сопоставляет книгу спотовой конверсии с целевыми направлениями.
//! Кто вызывает: пересборка индекса. Тик сразу использует готовую карту.
//! От чего зависит: рынки включённых сетей и рабочие направления того же контракта.
//! Настройки: сеть, валюты и включатели каждой площадки.
use super::{
    engine::{Core, rd},
    registry::CoinEntry,
    settlement_market::Market,
    types::{Asset, VenueId},
};
use std::collections::{HashMap, HashSet};
pub(super) fn books(core: &Core, coins: &[CoinEntry]) -> HashMap<(VenueId, String), Vec<Asset>> {
    let cexes: Vec<_> = rd(&core.inner.kinds)
        .iter()
        .filter(|(_, k)| *k == crate::gateway::contract::VenueKind::Cex)
        .map(|(id, _)| id.clone())
        .collect();
    let mut pairs = HashMap::new();
    for dex in super::chain_play::active_dexes(core) {
        for cex in &cexes {
            let Ok(m) = Market::new(core, &dex, cex) else {
                continue;
            };
            let key = m.book_key();
            pairs.insert((dex.clone(), cex.clone()), key.clone());
            pairs.insert((cex.clone(), dex.clone()), key);
        }
    }
    if pairs.is_empty() {
        return HashMap::new();
    }
    let mut watches: HashMap<_, HashSet<Asset>> = HashMap::new();
    for coin in coins {
        for d in coin.directions() {
            if let Some(key) = pairs.get(&(d.buy_on, d.sell_on)) {
                watches
                    .entry(key.clone())
                    .or_default()
                    .insert(coin.asset.clone());
            }
        }
    }
    watches
        .into_iter()
        .map(|(key, assets)| (key, assets.into_iter().collect()))
        .collect()
}
