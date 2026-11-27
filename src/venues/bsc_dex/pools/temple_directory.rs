//! The router has no enumerable list or addPair event. Probe the configured cash/assets and externally supplied pair seeds.
use super::super::{ctx::Ctx, directory::gt_parse::IndexedPool, node::rpc};
use super::{
    discover,
    families::{self, Family},
    family_variant::Variant,
    nami_batch as batch,
    stable_list::Extra,
    temple::{self, ITemple},
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
async fn discover_family(
    ctx: &Ctx,
    f: &Family,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    found: &mut Extra,
) -> GatewayResult<()> {
    let at = super::nami::block(ctx, "latest").await?;
    let router = ctx.dex_addr("dex.quoter", f.id)?;
    let [temple, defend] = batch::read(
        ctx,
        [
            batch::view(&at, router, ITemple::templeTokenCall {}),
            batch::view(&at, router, ITemple::defendStableCall {}),
        ],
    )
    .await?;
    let temple = batch::decode(&temple, ITemple::templeTokenCall {})?;
    let defend = batch::decode(&defend, ITemple::defendStableCall {})?;
    // Without TEMPLE in either side, none of this router's markets can contribute to this request.
    if !coins.contains_key(&temple) && !quotes.contains(&temple) {
        return Ok(());
    }
    let seeds = match ctx.dex_list("dex.stable_pools", f.id) {
        Ok(s) => s,
        Err(e) if e.code == ErrorCode::NotConfigured => Vec::new(),
        Err(e) => return Err(e),
    };
    let mut pools = BTreeSet::new();
    for s in seeds {
        if let Ok(a) = s.parse::<Address>()
            && !a.is_zero()
        {
            pools.insert(a);
        }
    }
    let currencies: BTreeSet<_> = quotes
        .iter()
        .chain(coins.keys())
        .copied()
        .chain([defend])
        .filter(|t| !t.is_zero() && *t != temple)
        .collect();
    let calls: Vec<_> = currencies
        .iter()
        .map(|s| batch::view(&at, router, ITemple::tokenPairCall { stable: *s }))
        .collect();
    for chunk in calls.chunks(100) {
        found.calls += chunk.len();
        for item in rpc::batch(ctx, chunk).await? {
            let raw: Bytes = serde_json::from_str(item?.get()).map_err(|_| {
                ctx.err(
                    ErrorCode::BadResponse,
                    "TempleDAO: tokenPair вернул неверные данные",
                )
            })?;
            let a = ITemple::tokenPairCall::abi_decode_returns_validate(&raw).ok();
            if let Some(a) = a.filter(|a| !a.is_zero()) {
                pools.insert(a);
            }
        }
    }
    for pool in pools {
        // A broken or retired seed may coexist with a live registered pair.
        let (state, stable) = match read(ctx, f, pool, temple, &at).await {
            Ok(v) => v,
            Err(e) if e.code == ErrorCode::NotApplicable => continue,
            Err(e) => {
                found.no_answer += 1;
                found.notes.push(format!("{} {pool}: {}", f.id, e.message));
                continue;
            }
        };
        let cash: BTreeSet<_> = [stable, state.defend]
            .into_iter()
            .filter(|t| !t.is_zero() && *t != temple)
            .collect();
        for token in cash {
            for (a, b) in [(temple, token), (token, temple)] {
                if let Some(symbol) = coins.get(&a)
                    && quotes.contains(&b)
                {
                    found.found.entry(a).or_default().push(IndexedPool {
                        dex_id: f.id.into(),
                        pool: pool.to_string(),
                        base: a,
                        quote: b,
                        base_symbol: symbol.clone(),
                        quote_symbol: coins.get(&b).cloned().unwrap_or_else(|| format!("{b:#x}")),
                        reserve_usd: None,
                    });
                }
            }
        }
    }
    Ok(())
}
async fn read(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    temple: Address,
    at: &Value,
) -> GatewayResult<(crate::core::types::TempleState, Address)> {
    let [raw] = batch::read(ctx, [batch::view(at, pool, ITemple::token1Call {})]).await?;
    let stable = batch::decode(&raw, ITemple::token1Call {})?;
    let state = temple::market(ctx, f, pool, (temple, stable), at).await?;
    Ok((state, stable))
}
pub(super) async fn scan(ctx: &Ctx, coins: &HashMap<Address, String>, quotes: &[Address]) -> Extra {
    let mut out = Extra::default();
    for f in families::all(ctx) {
        if f.variant == Variant::Temple
            && families::classify(ctx, f.id).is_ok()
            && let Err(e) = Box::pin(discover_family(ctx, f, coins, quotes, &mut out)).await
        {
            out.no_answer += 1;
            out.notes.push(format!("{}: {}", f.id, e.message));
        }
    }
    out
}
pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let Ok(pool) = c.pool.parse::<Address>() else {
        return Verdict::Rejected("TempleDAO: нужен адрес пары".into());
    };
    let capture = async {
        let at = super::nami::block(ctx, "latest").await?;
        temple::market(ctx, c.family, pool, (c.coin, c.quote), &at).await
    };
    match capture.await {
        Ok(_) => {
            let (token0, token1) = discover::sorted(c.coin, c.quote);
            Verdict::Taken(Box::new(super::verify_woo::found(
                c,
                PoolRef::Multi {
                    contract: pool,
                    token0,
                    token1,
                },
            )))
        }
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
