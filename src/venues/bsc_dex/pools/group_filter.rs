//! Файл: venues/bsc_dex/pools/group_filter.rs
//! Что делает: вид событий пачки (Topics), фильтр подписки logs пачки пулов одного вида (по адресам пулов /
//! контрактов или по PoolId у менеджера; Ekubo — ekubo_shallow.rs) и индексы пачки (ссылка → номер пула; адреса
//! контрактов пар многомонетных пулов). Вынесено из group.rs (решение 454 — Ekubo, решение 455 — файл вырос подпиской
//! Transfer, она — в transfer_in.rs); поведение прежнее, group.rs реэкспортирует всё под прежними путями (group::Topics
//! / filter / index / multi_sites).
//! Кто вызывает: pools/group.rs (subscribe), pools/session.rs (index, multi_sites; через group::),
//! tests/bsc_dex_receipts.rs, tests/bsc_dex_subs_live.rs (group::filter).
//! От чего зависит: node/subs.rs (фильтры), pools/events.rs (topic0), discover.rs (Found), ekubo_shallow.rs.
//! Настройки: нет.
//!
//! eth_subscribe("logs", фильтр) — execution-apis subscribe.yaml#L1-L60
//! (https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/subscribe.yaml#L1-L60).

use alloy_primitives::B256;
use serde_json::{Value, json};

use crate::core::types::PoolRef;

use super::super::node::subs;
use super::discover::Found;
use super::events;

/// Какие события слушать у пулов.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Topics {
    /// Все события, по которым ведётся состояние (events::topics).
    Full,
    /// Только добавление ликвидности (events::liquidity_topics).
    Liquidity,
}

/// Фильтр logs для пулов одного вида: V2/V3 — по адресам пулов, V4 — по PoolId у менеджера. Ошибок нет.
pub fn filter(pools: &[&Found], topics: Topics) -> Value {
    let Some(first) = pools.first() else {
        return json!({});
    };
    // Решение 447: Ekubo — события ядра без PoolId в topics (ekubo_shallow.rs filter).
    if let Some(f) = super::ekubo_shallow::filter(first, topics == Topics::Liquidity) {
        return f;
    }
    let topic0: Vec<B256> = match topics {
        Topics::Full => events::topics(first.family),
        Topics::Liquidity => events::liquidity_topics(first.family),
    };
    match first.reference {
        // Пара общего контракта (PMM от оракула, решение 122 F, 3) — по адресу общего контракта (события у него).
        // Решение 295: пары многомонетного пула — по адресу его контракта (одна строка адреса на все пары).
        PoolRef::Contract(_) | PoolRef::Member { .. } | PoolRef::Multi { .. } => {
            let mut addrs: Vec<_> = pools
                .iter()
                .filter_map(|f| match f.reference {
                    PoolRef::Contract(a) => Some(a),
                    PoolRef::Member { contract, .. } => Some(contract),
                    PoolRef::Multi { contract, .. } => Some(contract),
                    PoolRef::Singleton { .. } => None,
                })
                .collect();
            addrs.sort();
            addrs.dedup();
            subs::by_address(&addrs, &topic0)
        }
        PoolRef::Singleton { manager, .. } => {
            let ids: Vec<_> = pools
                .iter()
                .filter_map(|f| match f.reference {
                    PoolRef::Singleton { id, .. } => Some(id),
                    PoolRef::Contract(_) | PoolRef::Member { .. } | PoolRef::Multi { .. } => None,
                })
                .collect();
            subs::by_pool_id(manager, &ids, &topic0)
        }
    }
}

/// Индекс пулов пачки по ссылке (pools/session.rs: лог → пул). Ошибок нет.
pub fn index(pools: &[(Found, String)]) -> std::collections::HashMap<PoolRef, usize> {
    pools
        .iter()
        .enumerate()
        .map(|(i, (f, _))| (f.reference.clone(), i))
        .collect()
}

/// Адреса контрактов пар многомонетных пулов (PoolRef::Multi) пачки (решение 295, ревью): лог такого адреса идёт и в
/// listen_site.rs, даже если в пачке есть Contract того же адреса. Пусто — пар Multi нет. Ошибок нет.
pub fn multi_sites(
    pools: &[(Found, String)],
) -> std::collections::HashSet<alloy_primitives::Address> {
    pools
        .iter()
        .filter_map(|(f, _)| match f.reference {
            PoolRef::Multi { contract, .. } => Some(contract),
            _ => None,
        })
        .collect()
}
