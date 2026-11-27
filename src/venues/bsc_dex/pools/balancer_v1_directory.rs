//! BFactory's only CREATE is new BPool(). The contract nonce enumerates them all.
//! Persist finalized registry pages, revisit mutable token sets, and expose the live tail.
use super::{
    balancer_v1::{self as b, IBPool},
    families::{self, Family},
    family_variant::Variant,
    stable_list::{self, Extra},
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayResult},
    storage::venue_state,
    venues::bsc_dex::{
        ctx::Ctx,
        node::{call, rpc},
    },
};
use alloy_primitives::{Address, U64};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
const KIND: &str = "balancer_v1_directory";

#[derive(Clone, Serialize, Deserialize)]
struct Row {
    pool: Address,
    tokens: Vec<Address>,
    finalized: bool,
}
#[derive(Default, Serialize, Deserialize)]
struct Directory {
    pools: Vec<Row>,
    mutable_cursor: usize,
}

async fn nonce(ctx: &Ctx, factory: Address, at: &Value) -> GatewayResult<u64> {
    let n: U64 = rpc::call(ctx, "eth_getTransactionCount", json!([factory, at])).await?;
    Ok(n.to::<u64>())
}
async fn rows(
    ctx: &Ctx,
    factory: Address,
    pools: &[Address],
    at: &Value,
) -> GatewayResult<Vec<Row>> {
    let mut requests = Vec::with_capacity(pools.len() * 3);
    for &pool in pools {
        requests.extend([
            b::data(factory, IBPool::isBPoolCall { pool }),
            b::data(pool, IBPool::getCurrentTokensCall {}),
            b::data(pool, IBPool::isFinalizedCall {}),
        ]);
    }
    let raw = b::views(ctx, &requests, at).await?;
    pools
        .iter()
        .zip(raw.chunks_exact(3))
        .map(|(&pool, v)| {
            if !call::decode::<IBPool::isBPoolCall>(&v[0])? {
                return Err(b::bad(ctx));
            }
            let tokens = call::decode::<IBPool::getCurrentTokensCall>(&v[1])?;
            let finalized = call::decode::<IBPool::isFinalizedCall>(&v[2])?;
            if tokens.len() > 8 {
                return Err(b::bad(ctx));
            }
            Ok(Row {
                pool,
                tokens,
                finalized,
            })
        })
        .collect()
}
async fn family(
    ctx: &Ctx,
    f: &Family,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    extra: &mut Extra,
) -> GatewayResult<()> {
    let at = super::nami::block(ctx, "finalized").await?;
    let head = super::nami::block(ctx, "latest").await?;
    let factory = b::factory(ctx, f, &at).await?;
    b::factory(ctx, f, &head).await?;
    let (final_nonce, live_nonce) =
        tokio::try_join!(nonce(ctx, factory, &at), nonce(ctx, factory, &head))?;
    if live_nonce < final_nonce || final_nonce == 0 {
        return Err(b::bad(ctx));
    }
    let key = format!(
        "{}:{}:{factory:#x}:{}",
        ctx.chain_id().ok_or_else(|| b::bad(ctx))?,
        f.id,
        ctx.dex_text("dex.factory_runtime_hash", f.id)?
    );
    let db = ctx.journal.db();
    let err = |s: String| ctx.err(ErrorCode::Internal, format!("Balancer V1 registry: {s}"));
    let mut directory: Directory = match venue_state::load(db, ctx.place.id, KIND, &key)
        .await
        .map_err(|e| err(e.to_string()))?
    {
        Some(s) => serde_json::from_str(&s).map_err(|e| err(e.to_string()))?,
        None => Directory::default(),
    };
    let begin = 1 + directory.pools.len() as u64;
    if begin > final_nonce {
        return Err(b::bad(ctx));
    }
    let width = super::load_batch::batch(ctx)?.min(1024);
    let fresh: Vec<_> = (begin..final_nonce)
        .take(width)
        .map(|i| factory.create(i))
        .collect();
    let mutable: Vec<_> = directory
        .pools
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.finalized)
        .map(|(i, _)| i)
        .collect();
    let revisit: Vec<_> = if mutable.is_empty() {
        vec![]
    } else {
        (0..width.min(mutable.len()))
            .map(|k| mutable[(k + directory.mutable_cursor) % mutable.len()])
            .collect()
    };
    let mut changes = fresh.clone();
    changes.extend(revisit.iter().map(|&i| directory.pools[i].pool));
    if !changes.is_empty() {
        let mut updated = rows(ctx, factory, &changes, &at).await?.into_iter();
        directory.pools.extend(updated.by_ref().take(fresh.len()));
        for (&i, pool) in revisit.iter().zip(updated) {
            directory.pools[i] = pool;
        }
        if !mutable.is_empty() {
            directory.mutable_cursor = (directory.mutable_cursor + revisit.len()) % mutable.len();
        }
        venue_state::save(
            db,
            ctx.place.id,
            KIND,
            &key,
            serde_json::to_string(&directory).map_err(|e| err(e.to_string()))?,
        )
        .await
        .map_err(|e| err(e.to_string()))?;
    }
    // The volatile tail never enters the finalized cursor. A reorg rederives it by nonce.
    let recent: Vec<_> = (final_nonce..live_nonce)
        .map(|i| factory.create(i))
        .collect();
    let current = rows(ctx, factory, &recent, &head).await?;
    extra.calls += (recent.len() + changes.len()) * 3 + 2;
    for r in directory.pools.iter().chain(&current) {
        for row in stable_list::candidates(f.id, r.pool, &r.tokens, coins, quotes) {
            extra.found.entry(row.base).or_default().push(row);
        }
    }
    Ok(())
}
pub(super) async fn scan(ctx: &Ctx, coins: &HashMap<Address, String>, quotes: &[Address]) -> Extra {
    let mut extra = Extra::default();
    for f in families::all(ctx) {
        if f.variant == Variant::BalancerV1
            && families::classify(ctx, f.id).is_ok()
            && let Err(e) = Box::pin(family(ctx, f, coins, quotes, &mut extra)).await
        {
            extra.notes.push(e.message);
            extra.no_answer += 1;
        }
    }
    extra
}
pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let Ok(pool) = c.pool.parse::<Address>() else {
        return Verdict::Rejected("Balancer V1: неверный адрес BPool".into());
    };
    let result: GatewayResult<PoolRef> = async {
        let at = super::nami::block(ctx, "latest").await?;
        let s = b::load(ctx, c.family, pool, &at).await?;
        let (token0, token1) = super::discover::sorted(c.coin, c.quote);
        if token0 == token1
            || ![token0, token1]
                .iter()
                .all(|t| s.assets.iter().any(|a| a.token == *t))
        {
            return Err(b::bad(ctx));
        }
        Ok(PoolRef::Multi {
            contract: pool,
            token0,
            token1,
        })
    }
    .await;
    match result {
        Ok(r) => Verdict::Taken(Box::new(super::verify_woo::found(c, r))),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
