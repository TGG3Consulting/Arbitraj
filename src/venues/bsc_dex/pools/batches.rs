//! Файл: venues/bsc_dex/pools/batches.rs
//! Что делает: раскладка пулов по пачкам подписки: по виду (и менеджеру V4) — у каждого вида свои темы событий, внутри —
//! не больше dex.pools_per_sub пулов в пачке. Пулы без тикера монеты не слушаются.
//! Кто вызывает: pools/market.rs (стартовые пачки), attach.rs (пулы, найденные позже).
//! От чего зависит: discover.rs (Found). Решение 295: пары многомонетного пула одного контракта — в одной пачке (pack).
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
            // Пары общего контракта (PMM от оракула) — одна пачка на контракт (общее чтение на каждом блоке).
            PoolRef::Member { contract, .. } => Some(*contract),
            // Решение 295: пары многомонетного пула — в пачке вида вместе с другими пулами (не своя пачка на
            // контракт); при нарезке пары одного контракта не разрываются (chunks → multi_pair::pack).
            PoolRef::Contract(_) | PoolRef::Multi { .. } => None,
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
        // Решение 295: в пачке есть пары многомонетных пулов — пары одного контракта в одной пачке (одно чтение на
        // контракт); иначе — прежняя нарезка подряд.
        let parts: Vec<Tickered> = if list.iter().any(|(f, _)| matches!(f.reference, PoolRef::Multi { .. })) {
            pack(list, per)
        } else {
            list.chunks(per).map(<[_]>::to_vec).collect()
        };
        for (i, part) in parts.into_iter().enumerate() {
            out.push((format!("{family}-{i}"), part));
        }
    }
    out
}

/// Что делает: нарезка пачки подписки, где есть пары Multi: пары одного контракта идут подряд и в одну пачку (одно
/// чтение и одна подписка на контракт); пачка — не больше `per` пулов; контракт с парами больше `per` — режется.
/// Порядок — первого появления контракта. Принимает: пулы с тикерами, размер пачки (> 0). Возвращает: пачки.
/// Ошибок нет.
fn pack(list: Tickered, per: usize) -> Vec<Tickered> {
    let per = per.max(1);
    let mut order: Vec<PoolRef> = Vec::new();
    let mut by: std::collections::HashMap<PoolRef, Tickered> = Default::default();
    for x in list {
        let site = x.0.reference.site();
        if !by.contains_key(&site) {
            order.push(site.clone());
        }
        by.entry(site).or_default().push(x);
    }
    let mut out: Vec<Tickered> = Vec::new();
    let mut cur: Tickered = Vec::new();
    for site in order {
        let group = by.remove(&site).unwrap_or_default();
        if !cur.is_empty() && cur.len() + group.len() > per {
            out.push(std::mem::take(&mut cur));
        }
        for x in group {
            if cur.len() == per {
                out.push(std::mem::take(&mut cur));
            }
            cur.push(x);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
