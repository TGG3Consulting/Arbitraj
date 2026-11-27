//! Enumerate physical LLAMMA registries in bounded batches only during directory scans.
//! NewVault/AddMarket supply the specific new index. Admission rechecks its canonical factory row.
use super::{
    balancer_v1::views,
    families::{self, Family},
    family_variant::Variant,
    llamma::{self as l, ILlama, Registry},
    stable_list::{self, Extra},
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::call},
};
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256};
use std::collections::HashMap;
async fn family(
    ctx: &Ctx,
    f: &Family,
    kind: Registry,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    extra: &mut Extra,
) -> GatewayResult<()> {
    let at = super::nami::block(ctx, "latest").await?;
    let fac = l::factory(ctx, f, &at).await?;
    let answer = views(ctx, &[kind.count(fac)], &at).await?;
    let count = u64::try_from(call::decode::<ILlama::market_countCall>(&answer[0])?)
        .map_err(|_| l::bad(ctx))?;
    let page = super::load_batch::batch(ctx)?;
    let width = kind.ask(fac, 0).len();
    extra.calls += 1;
    for begin in (0..count).step_by(page) {
        let requests = (begin..count.min(begin.saturating_add(page as u64)))
            .flat_map(|i| kind.ask(fac, i))
            .collect::<Vec<_>>();
        let rows = super::balancer_v1::individual(ctx, &requests, &at).await?;
        extra.calls += requests.len();
        for (offset, chunk) in rows.chunks_exact(width).enumerate() {
            let Some(values) = chunk
                .iter()
                .map(|r| r.as_ref().ok().cloned())
                .collect::<Option<Vec<_>>>()
            else {
                extra.no_answer += 1;
                continue;
            };
            let Ok(m) = kind.row(ctx, begin + offset as u64, &values) else {
                extra.no_answer += 1;
                continue;
            };
            for item in
                stable_list::candidates(f.id, l::key(m.index, m.amm), &m.coins, coins, quotes)
            {
                extra.found.entry(item.base).or_default().push(item)
            }
        }
    }
    Ok(())
}
pub(super) async fn scan(ctx: &Ctx, coins: &HashMap<Address, String>, quotes: &[Address]) -> Extra {
    let mut extra = Extra::default();
    for f in families::all(ctx) {
        if let Variant::Llamma(kind) = f.variant
            && families::classify(ctx, f.id).is_ok()
            && let Err(e) = Box::pin(family(ctx, f, kind, coins, quotes, &mut extra)).await
        {
            extra.notes.push(e.message);
            extra.no_answer += 1;
        }
    }
    extra
}
pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let result: GatewayResult<PoolRef> = async {
        let id = c.pool.parse::<B256>().map_err(|_| l::bad(ctx))?;
        let (index, venue) = l::decode_key(id).ok_or_else(|| l::bad(ctx))?;
        let at = super::nami::block(ctx, "latest").await?;
        let (_, m, _) = l::bound(ctx, c.family, index, &at).await?;
        if venue != m.amm
            || c.quote == c.coin
            || !m.coins.contains(&c.quote)
            || !m.coins.contains(&c.coin)
        {
            return Err(l::bad(ctx));
        }
        Ok(PoolRef::Singleton { manager: m.amm, id })
    }
    .await;
    match result {
        Ok(r) => Verdict::Taken(Box::new(super::verify_woo::found(c, r))),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
pub(super) async fn created(
    ctx: &Ctx,
    f: &'static Family,
    log: &alloy::rpc::types::Log,
    quote: Address,
) -> GatewayResult<Option<Candidate>> {
    let (id, amm) = if f.variant == Variant::Llamma(Registry::Mint) {
        let e = ILlama::AddMarket::decode_log_data(&log.inner.data).map_err(|_| l::bad(ctx))?;
        (e.ix, e.amm)
    } else {
        let e = ILlama::NewVault::decode_log_data(&log.inner.data).map_err(|_| l::bad(ctx))?;
        (e.id, e.amm)
    };
    let index = u64::try_from(id).map_err(|_| l::bad(ctx))?;
    let at = serde_json::json!({"blockHash":log.block_hash.ok_or_else(||l::bad(ctx))?,"requireCanonical":true});
    let (factory, m, _) = l::bound(ctx, f, index, &at).await?;
    if factory != log.address() || m.amm != amm {
        return Err(l::bad(ctx));
    }
    Ok(m.coins.iter().position(|a| *a == quote).map(|i| Candidate {
        family: f,
        coin: m.coins[1 - i],
        quote,
        pool: format!("{:#x}", l::key(index, amm)),
    }))
}
