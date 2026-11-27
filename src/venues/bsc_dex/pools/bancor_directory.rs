//! Discover both raw ERC20 and native variants from the full on-chain Network registry.
use super::{
    bancor_v3::{self, IBancor},
    families::{self, Family},
    family_variant::Variant,
    nami_batch as batch,
    stable_list::Extra,
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, directory::gt_parse::IndexedPool},
};
use alloy_primitives::Address;
use std::collections::{HashMap, HashSet};
fn pair(a: Address, b: Address) -> String {
    let (l, r) = super::discover::sorted(a, b);
    format!("{l:#x}/{r:#x}")
}
fn variants(t: Address, wrapped: Address, native: Address) -> impl Iterator<Item = Address> {
    std::iter::once(t).chain((t == wrapped).then_some(native))
}
async fn family(
    ctx: &Ctx,
    f: &Family,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    extra: &mut Extra,
) -> GatewayResult<()> {
    let at = super::nami::block(ctx, "latest").await?;
    let r = bancor_v3::roles(ctx, f, &at).await?;
    let [v] = batch::read(
        ctx,
        [batch::view(&at, r.network, IBancor::liquidityPoolsCall {})],
    )
    .await?;
    let mut registered: HashSet<_> = batch::decode(&v, IBancor::liquidityPoolsCall {})?
        .into_iter()
        .collect();
    registered.insert(r.bnt);
    extra.calls += 1;
    for (&coin, name) in coins {
        for &quote in quotes.iter().filter(|q| **q != coin) {
            for a in variants(coin, r.wrapped, r.native).filter(|t| registered.contains(t)) {
                for b in variants(quote, r.wrapped, r.native).filter(|t| registered.contains(t)) {
                    extra.found.entry(coin).or_default().push(IndexedPool {
                        dex_id: f.id.into(),
                        pool: pair(a, b),
                        base: coin,
                        quote,
                        base_symbol: name.clone(),
                        quote_symbol: coins
                            .get(&quote)
                            .cloned()
                            .unwrap_or_else(|| format!("{quote:#x}")),
                        reserve_usd: None,
                    });
                }
            }
        }
    }
    Ok(())
}
pub(super) async fn scan(ctx: &Ctx, coins: &HashMap<Address, String>, quotes: &[Address]) -> Extra {
    let mut extra = Extra::default();
    for f in families::all(ctx) {
        if f.variant == Variant::BancorV3
            && families::classify(ctx, f.id).is_ok()
            && let Err(e) = Box::pin(family(ctx, f, coins, quotes, &mut extra)).await
        {
            extra.notes.push(format!("{}: {}", f.id, e.message));
            extra.no_answer += 1;
        }
    }
    extra
}
pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let Some((a, b)) = c.pool.split_once('/') else {
        return Verdict::Rejected("Bancor V3: ожидаются две физические валюты".into());
    };
    let (Ok(token0), Ok(token1)) = (a.parse(), b.parse()) else {
        return Verdict::Rejected("Bancor V3: неверные адреса".into());
    };
    let capture: GatewayResult<PoolRef> = async {
        let reference = PoolRef::Multi {
            contract: ctx.dex_addr("dex.factory", c.family.id)?,
            token0,
            token1,
        };
        let (a, b) = super::discover::sorted(c.coin, c.quote);
        let at = super::nami::block(ctx, "latest").await?;
        bancor_v3::load(ctx, c.family, &reference, [a, b], &at).await?;
        Ok(reference)
    }
    .await;
    match capture {
        Ok(r) => Verdict::Taken(Box::new(super::verify_woo::found(c, r))),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
pub(super) fn created(
    ctx: &Ctx,
    f: &'static Family,
    raw: Address,
    watched: impl Iterator<Item = Address>,
    quote: Address,
) -> GatewayResult<Vec<Candidate>> {
    let wrapped = ctx.addr("dex.native_wrapped")?;
    let native = ctx.dex_addr("dex.bancor_native_token", f.id)?;
    let asset = if raw == native { wrapped } else { raw };
    let mut out = Vec::new();
    for coin in watched.filter(|c| *c != quote && (*c == asset || quote == asset)) {
        let other = if coin == asset { quote } else { coin };
        for t in variants(other, wrapped, native) {
            out.push(Candidate {
                family: f,
                pool: pair(raw, t),
                coin,
                quote,
            });
        }
    }
    Ok(out)
}
