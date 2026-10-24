//! Файл: venues/bsc_dex/pools/batches.rs
//! Что делает: раскладка пулов по пачкам подписки: по виду (и менеджеру V4) — у каждого вида свои темы событий, внутри —
//! не больше dex.pools_per_sub пулов в пачке. Пулы без тикера монеты не слушаются.
//! Кто вызывает: pools/market.rs (стартовые пачки), attach.rs (пулы, найденные позже).
//! От чего зависит: discover.rs (Found).
//! Настройки: нет своих (размер пачки приходит параметром).

use std::collections::HashMap;

use alloy_primitives::Address;

use crate::core::types::PoolRef;

use super::discover::Found;

/// Пулы пачки с тикерами монет.
pub type Tickered = Vec<(Found, String)>;

/// Пулы одного вида (и менеджера V4) с тикерами: (ключ вида, пулы). Пулы без тикера монеты не слушаются. Ошибок нет.
pub fn by_family(
    found: &[Found],
    coins: &HashMap<Address, String>,
) -> Vec<(String, &'static str, Tickered)> {
    let mut groups: HashMap<(&'static str, Option<Address>), Tickered> = HashMap::new();
    for f in found {
        let manager = match &f.reference {
            PoolRef::Singleton { manager, .. } => Some(*manager),
            PoolRef::Contract(_) => None,
        };
        // Пулы найдены по монетам подписки, у каждой есть тикер; без тикера пул не слушается.
        if let Some(t) = coins.get(&f.coin) {
            groups
                .entry((f.family.id, manager))
                .or_default()
                .push((f.clone(), t.clone()));
        }
    }
    groups
        .into_iter()
        .map(|((family, manager), list)| (format!("{family}:{manager:?}"), family, list))
        .collect()
}

/// Пачки для подписки: (имя, пулы с тикерами), не больше `per` пулов в пачке. Ошибок нет.
pub fn chunks(
    found: &[Found],
    coins: &HashMap<Address, String>,
    per: usize,
) -> Vec<(String, Vec<(Found, String)>)> {
    let mut out = Vec::new();
    for (_, family, list) in by_family(found, coins) {
        for (i, part) in list.chunks(per).enumerate() {
            out.push((format!("{family}-{i}"), part.to_vec()));
        }
    }
    out
}
