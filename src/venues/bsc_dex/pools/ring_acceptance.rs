//! Проверка текущего профиля Ring перед кодированием обмена. Пул допускается
//! после сверки контрактов на общем каноническом блоке; смена адреса, кода,
//! сети или обёртки лишает старую запись силы.
use super::ring::{self, Layout};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::ctx::Ctx;
use alloy_primitives::{Address, B256, U64};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, Eq, PartialEq, serde::Deserialize)]
pub(super) struct Anchor {
    pub hash: B256,
    pub number: U64,
}

struct Accepted {
    chain: u64,
    layout: Layout,
    final_height: Option<U64>,
    last_probe: Option<Instant>,
    probing: bool,
    pools: HashMap<Address, Option<Anchor>>,
}

type Scope = (&'static str, &'static str);
#[derive(Default)]
pub(in crate::venues::bsc_dex) struct Book(Mutex<HashMap<Scope, Accepted>>);

pub(super) fn record(
    ctx: &Ctx,
    id: &'static str,
    layout: &Layout,
    pool: Address,
    chain: u64,
    anchor: Anchor,
) {
    let mut map = ctx
        .evm_chain
        .ring_proofs()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let value = map.entry((ctx.place.id, id)).or_insert_with(|| Accepted {
        chain,
        layout: layout.clone(),
        final_height: None,
        last_probe: None,
        probing: false,
        pools: HashMap::new(),
    });
    if value.chain != chain || value.layout != *layout {
        *value = Accepted {
            chain,
            layout: layout.clone(),
            final_height: None,
            last_probe: None,
            probing: false,
            pools: HashMap::new(),
        };
    }
    value.pools.entry(pool).or_insert(Some(anchor));
}

/// Отзыв ранее принятой пары или всего профиля после положительного доказательства расхождения.
pub(super) fn revoke(ctx: &Ctx, id: &'static str, layout: &Layout, pool: Option<Address>) {
    let mut map = ctx
        .evm_chain
        .ring_proofs()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(v) = map.get_mut(&(ctx.place.id, id))
        && &v.layout == layout
    {
        if let Some(pool) = pool {
            v.pools.remove(&pool);
        } else {
            v.pools.clear();
        }
    }
}

/// Для всей DEX читаем финальность в фоне. Для новых пар канонический
/// hash проверяется при обращении и до первого ответа о финальности.
pub(super) fn final_known(ctx: &Ctx, id: &'static str, layout: &Layout, anchor: Anchor) -> bool {
    let mut map = ctx
        .evm_chain
        .ring_proofs()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(v) = map.get_mut(&(ctx.place.id, id)) else {
        return false;
    };
    if v.layout != *layout || Some(v.chain) != ctx.int("dex.evm_chain_id").ok() {
        return false;
    }
    if v.final_height.is_some_and(|n| n >= anchor.number) {
        return true;
    }
    // Одна фоновая проверка на все пары, максимум раз за слот Ethereum.
    if !v.probing
        && v.last_probe
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(12))
    {
        v.probing = true;
        v.last_probe = Some(Instant::now());
        let ctx = ctx.clone();
        let chain = v.chain;
        let layout = layout.clone();
        tokio::spawn(async move {
            let observed = crate::venues::bsc_dex::node::rpc::call::<Anchor>(
                &ctx,
                "eth_getBlockByNumber",
                serde_json::json!(["finalized", false]),
            )
            .await;
            let mut map = ctx
                .evm_chain
                .ring_proofs()
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(v) = map.get_mut(&(ctx.place.id, id))
                && v.chain == chain
                && v.layout == layout
            {
                v.probing = false;
                if let Ok(head) = observed {
                    v.final_height = Some(head.number);
                }
            }
        });
    }
    false
}
pub(super) enum Outcome {
    Orphaned,
    Finalized,
}
impl Outcome {
    fn apply(self, pools: &mut HashMap<Address, Option<Anchor>>, pool: Address, expected: Anchor) {
        if pools.get(&pool) != Some(&Some(expected)) {
            return;
        }
        match self {
            Self::Orphaned => {
                pools.remove(&pool);
            }
            Self::Finalized => {
                pools.insert(pool, None);
            }
        }
    }
}
pub(super) fn settle(
    ctx: &Ctx,
    id: &'static str,
    layout: &Layout,
    pool: Address,
    old: Anchor,
    result: Outcome,
) {
    let mut map = ctx
        .evm_chain
        .ring_proofs()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(v) = map.get_mut(&(ctx.place.id, id))
        && v.layout == *layout
        && Some(v.chain) == ctx.int("dex.evm_chain_id").ok()
    {
        result.apply(&mut v.pools, pool, old);
    }
}

pub(super) fn lookup(
    ctx: &Ctx,
    id: &'static str,
    pool: Address,
) -> GatewayResult<(Layout, Option<Anchor>)> {
    let layout = ring::active(ctx, id)?;
    let map = ctx
        .evm_chain
        .ring_proofs()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let proof = map
        .get(&(ctx.place.id, id))
        .filter(|v| Some(v.chain) == ctx.int("dex.evm_chain_id").ok() && v.layout == layout)
        .and_then(|v| v.pools.get(&pool));
    if let Some(anchor) = proof {
        Ok((layout, *anchor))
    } else {
        Err(ctx.err(
            ErrorCode::NoRoute,
            "Ring: пул с текущими настройками ещё не проверен на узле",
        ))
    }
}

pub fn checked(ctx: &Ctx, id: &'static str, pool: Address) -> GatewayResult<Layout> {
    lookup(ctx, id, pool).map(|(layout, _)| layout)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_finality_or_reorg_cannot_overwrite_the_new_branch_at_the_same_height() {
        let pool = Address::with_last_byte(7);
        let a = Anchor {
            hash: B256::with_last_byte(1),
            number: U64::from(42),
        };
        let b = Anchor {
            hash: B256::with_last_byte(2),
            number: a.number,
        };
        let mut pools = HashMap::from([(pool, Some(a))]);
        Outcome::Orphaned.apply(&mut pools, pool, a);
        assert!(!pools.contains_key(&pool));
        pools.insert(pool, Some(b));
        Outcome::Finalized.apply(&mut pools, pool, a);
        Outcome::Orphaned.apply(&mut pools, pool, a);
        assert_eq!(pools[&pool], Some(b));
        Outcome::Finalized.apply(&mut pools, pool, b);
        Outcome::Orphaned.apply(&mut pools, pool, b);
        assert_eq!(pools[&pool], None);
    }
}
