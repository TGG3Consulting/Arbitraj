//! The original singleton publishes its DexKey array in storage slot 1. A reloaded key is checked against both its position and hash.
use super::{
    families::{self, Family},
    fluid_dex_lite as lite,
    stable_list::{self, Extra},
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{Address, B256, U256};
use std::collections::HashMap;
pub(super) async fn scan(
    c: &Ctx,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    extra: &mut Extra,
) {
    for f in families::all(c)
        .into_iter()
        .filter(|f| f.variant.fluid_lite())
    {
        if let Err(e) = scan_one(c, f, coins, quotes, extra).await {
            extra.no_answer += 1;
            extra.notes.push(e.message);
        }
    }
}
async fn scan_one(
    c: &Ctx,
    f: &Family,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    extra: &mut Extra,
) -> GatewayResult<()> {
    let at = super::nami::block(c, "latest").await?;
    let role = lite::bind(c, f, &at).await?;
    let n = usize::try_from(lite::count(c, role.core, &at).await?).map_err(|_| lite::bad(c))?;
    let wrapped = c.addr("dex.native_wrapped")?;
    let chunk = super::load_batch::batch(c)?.max(1);
    for start in (0..n).step_by(chunk) {
        let idx: Vec<_> = (start..start.saturating_add(chunk).min(n))
            .map(U256::from)
            .collect();
        for m in lite::keys(c, role.core, &idx, &at).await? {
            let Some(tokens) = m.tokens(wrapped) else {
                continue;
            };
            let Some(id) = m.id() else { continue };
            for entry in stable_list::candidates(f.id, id, &tokens, coins, quotes) {
                extra.found.entry(entry.base).or_default().push(entry);
            }
        }
        extra.calls += 3 * idx.len();
    }
    Ok(())
}
pub(super) async fn verify(c: &Ctx, v: &Candidate) -> Verdict {
    let result = async {
        let at = super::nami::block(c, "latest").await?;
        let r = lite::bind(c, v.family, &at).await?;
        let id = v.pool.parse::<B256>().map_err(|_| lite::bad(c))?;
        let m = lite::resolve(c, r.core, id, &at).await?;
        let pair = m
            .tokens(c.addr("dex.native_wrapped")?)
            .ok_or_else(|| lite::bad(c))?;
        if v.coin == v.quote || !pair.contains(&v.coin) || !pair.contains(&v.quote) {
            return Err(lite::bad(c));
        }
        Ok(PoolRef::Singleton {
            manager: r.core,
            id,
        })
    }
    .await;
    match result {
        Ok(r) => Verdict::Taken(Box::new(super::verify_woo::found(v, r))),
        Err(e) if e.code == ErrorCode::QuoteMismatch => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
