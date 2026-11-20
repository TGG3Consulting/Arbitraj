//! Файл: venues/bsc_dex/pools/ekubo_shallow.rs
//! Что делает: подписка на события пулов Ekubo (решение 447). У ядра PoolId не в topics, а обмен — анонимный log0 без
//! topics: фильтр «PoolId в topics[1]» (V4) и «topic0 из списка» его не пропустят. Поэтому:
//! - подписка «все события» пачки Ekubo — по адресу ядра без topics (все логи ядра; пул выбирает ekubo_route.rs);
//! - подписка «только ликвидность» — по адресу ядра и topic0 PositionUpdated;
//! - лог «всех событий» до мелкого пула того же ядра будит его только событием ликвидности (как мелкий V2 — Mint).
//!
//! Решение 454: подписка «все события» — по адресам ядра и TWAMM DEX (dex.ekubo_twamm: события TWAMM меняют пул);
//! при источнике ws пачка Ekubo не держит своей подписки — общая подписка площадки (ekubo_hub.rs), пачке — логи её
//! пулов (plan: адреса, PoolId пулов, «topic0» подписки «только ликвидность»).
//! Кто вызывает: pools/group.rs (filter, plan), pools/session.rs (wakes).
//! От чего зависит: families.rs, events.rs, ekubo_route.rs, ekubo_ext.rs, discover.rs (Found), ctx.rs.
//! Настройки: через ekubo_ext.rs — dex.ekubo_twamm [DEX].
//!
//! Фильтр logs: address — адрес; topics не задан — любые логи адреса (execution-apis
//! 16caee274b0d4fb7a0316f294029adac88a75a09 src/eth/subscribe.yaml L1-L60, тот же фильтр, что у eth_getLogs).

use std::collections::HashSet;

use alloy::rpc::types::Log;
use alloy_primitives::{Address, B256};
use serde_json::{Value, json};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::discover::Found;
use super::events;
use super::families::{Family, Proto};

/// Подписка пачки Ekubo: адреса (ядро и TWAMM DEX), PoolId пулов пачки, «topic0» (None — все события).
pub struct Plan {
    pub addrs: Vec<Address>,
    pub ids: HashSet<B256>,
    pub topics: Option<Vec<B256>>,
}

/// План подписки пачки Ekubo. Принимает: окружение, пулы пачки (одного вида), только ли ликвидность. Возвращает: None —
/// пачка не Ekubo. Ошибок нет.
pub fn plan(ctx: &Ctx, pools: &[&Found], liquidity: bool) -> Option<Plan> {
    let first = pools.first()?;
    if first.family.proto != Proto::Ekubo {
        return None;
    }
    let PoolRef::Singleton { manager, .. } = first.reference else {
        return None;
    };
    let mut addrs = vec![manager];
    addrs.extend(super::ekubo_ext::twamm_list(ctx, first.family.id));
    let ids = pools
        .iter()
        .filter_map(|f| match f.reference {
            PoolRef::Singleton { id, .. } => Some(id),
            _ => None,
        })
        .collect();
    let topics = liquidity.then(|| events::liquidity_topics(first.family));
    Some(Plan { addrs, ids, topics })
}

/// Фильтр подписки пачки Ekubo без хаба (лента квитанций): «все события» — адреса плана без topics, «только
/// ликвидность» — ядро и topic0 PositionUpdated. Принимает: план, ядро. Ошибок нет.
pub fn filter_of(plan: &Plan, core: Address) -> Value {
    match &plan.topics {
        Some(t) => json!({ "address": core, "topics": [t] }),
        None => json!({ "address": plan.addrs }),
    }
}

/// Фильтр подписки пачки Ekubo по одному ядру (без TWAMM). Принимает: первый пул пачки (все пулы пачки одного вида),
/// только ли ликвидность. Возвращает: None — пачка не Ekubo (общий путь group.rs). Ошибок нет.
pub fn filter(first: &Found, liquidity: bool) -> Option<Value> {
    if first.family.proto != Proto::Ekubo {
        return None;
    }
    let PoolRef::Singleton { manager, .. } = first.reference else {
        return None;
    };
    Some(if liquidity {
        json!({ "address": manager, "topics": [events::liquidity_topics(first.family)] })
    } else {
        json!({ "address": manager })
    })
}

/// Будит ли лог мелкий пул: у Ekubo — только событие ликвидности (подписка «все события» соседей по ядру приносит
/// и обмены); у других видов — как раньше (любой лог их подписки ликвидности). Ошибок нет.
pub fn wakes(f: &Family, log: &Log) -> bool {
    if f.proto != Proto::Ekubo {
        return true;
    }
    super::ekubo_route::topic(log).is_some_and(|t| events::liquidity_topics(f).contains(&t))
}
