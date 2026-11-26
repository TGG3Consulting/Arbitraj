//! Проверка текущего профиля Ring перед кодированием обмена. Пул допускается
//! после сверки контрактов на общем каноническом блоке; смена адреса, кода,
//! сети или обёртки лишает старую запись силы.
use super::ring::{self, Layout};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::ctx::Ctx;
use alloy_primitives::Address;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

struct Accepted {
    chain: u64,
    layout: Layout,
    pools: HashSet<Address>,
}

type Scope = (&'static str, &'static str);
#[derive(Default)]
pub(in crate::venues::bsc_dex) struct Book(Mutex<HashMap<Scope, Accepted>>);

pub(super) fn record(ctx: &Ctx, id: &'static str, layout: &Layout, pool: Address, chain: u64) {
    let mut map = ctx
        .evm_chain
        .ring_proofs()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let value = map.entry((ctx.place.id, id)).or_insert_with(|| Accepted {
        chain,
        layout: layout.clone(),
        pools: HashSet::new(),
    });
    if value.chain != chain || value.layout != *layout {
        *value = Accepted {
            chain,
            layout: layout.clone(),
            pools: HashSet::new(),
        };
    }
    value.pools.insert(pool);
}

pub fn checked(ctx: &Ctx, id: &'static str, pool: Address) -> GatewayResult<Layout> {
    let layout = ring::active(ctx, id)?;
    let map = ctx
        .evm_chain
        .ring_proofs()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let valid = map.get(&(ctx.place.id, id)).is_some_and(|v| {
        Some(v.chain) == ctx.int("dex.evm_chain_id").ok()
            && v.layout == layout
            && v.pools.contains(&pool)
    });
    if valid {
        Ok(layout)
    } else {
        Err(ctx.err(
            ErrorCode::NoRoute,
            "Ring: пул с текущими настройками ещё не проверен на узле",
        ))
    }
}
