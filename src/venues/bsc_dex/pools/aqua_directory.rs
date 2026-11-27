//! Поиск программы и конкретной пары без коммерческого индексатора.
use super::{
    aqua::{self, Program, Roles},
    aqua_book::{self, Book, Offer},
    families::{self, Family},
    family_variant::Variant,
    stable_list::{self, Extra},
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy::sol_types::SolValue;
use alloy_primitives::{Address, B256, Bytes, keccak256};
use serde_json::Value;
use std::collections::HashMap;

fn scope(ctx: &Ctx, f: &Family) -> GatewayResult<(u64, Roles)> {
    Ok((
        ctx.chain_id().ok_or_else(|| aqua::bad(ctx))?,
        Roles {
            registry: ctx.dex_addr("dex.factory", f.id)?,
            engine: ctx.dex_addr("dex.quoter", f.id)?,
        },
    ))
}
fn seeds(ctx: &Ctx, f: &Family) -> GatewayResult<Book> {
    let mut b = Book::default();
    let entries = match ctx.dex_list("dex.aqua_programs", f.id) {
        Ok(list) => list,
        Err(e) if e.code == ErrorCode::NotConfigured => vec![],
        Err(e) => return Err(e),
    };
    for s in entries {
        let bytes = s.parse::<Bytes>().map_err(|_| aqua::bad(ctx))?;
        let (p, tokens) = <(Program, Vec<Address>)>::abi_decode_params_validate(&bytes)
            .map_err(|_| aqua::bad(ctx))?;
        let raw = Bytes::from(p.abi_encode());
        let h = keccak256(&raw);
        if !b.insert(p.maker, h, raw) {
            return Err(aqua::bad(ctx));
        }
        b.tokens(p.maker, h, &tokens);
    }
    Ok(b)
}
pub(in crate::venues::bsc_dex) fn identity(
    ctx: &Ctx,
    f: &Family,
    r: &PoolRef,
    pair: (Address, Address),
) -> GatewayResult<Offer> {
    let (chain, roles) = scope(ctx, f)?;
    let PoolRef::Singleton { manager, id } = r else {
        return Err(aqua::bad(ctx));
    };
    if *manager != roles.engine {
        return Err(aqua::bad(ctx));
    }
    if let Some(t) = aqua_book::current(chain, roles).offer(*id, pair) {
        return Ok(t);
    }
    seeds(ctx, f)?
        .offer(*id, pair)
        .ok_or_else(|| aqua::bad(ctx))
}
pub(in crate::venues::bsc_dex) async fn checked(
    ctx: &Ctx,
    f: &Family,
    r: &PoolRef,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<(Roles, Offer)> {
    let roles = Roles::checked(ctx, f, at).await?;
    let offer = identity(ctx, f, r, pair)?;
    roles
        .active(ctx, &offer.program, offer.hash, pair, at)
        .await?;
    Ok((roles, offer))
}
pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let result: GatewayResult<PoolRef> = async {
        let (_, roles) = scope(ctx, c.family)?;
        let h = c.pool.parse::<B256>().map_err(|_| aqua::bad(ctx))?;
        let r = PoolRef::Singleton {
            manager: roles.engine,
            id: h,
        };
        let at = super::nami::block(ctx, "latest").await?;
        checked(
            ctx,
            c.family,
            &r,
            super::discover::sorted(c.coin, c.quote),
            &at,
        )
        .await?;
        Ok(r)
    }
    .await;
    match result {
        Ok(r) => Verdict::Taken(Box::new(super::verify_woo::found(c, r))),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
pub(super) fn scan(ctx: &Ctx, coins: &HashMap<Address, String>, quotes: &[Address]) -> Extra {
    let mut out = Extra::default();
    for f in families::all(ctx)
        .into_iter()
        .filter(|f| f.variant == Variant::Aqua && families::classify(ctx, f.id).is_ok())
    {
        let Ok((chain, roles)) = scope(ctx, f) else {
            continue;
        };
        let book = aqua_book::current(chain, roles);
        let seed = match seeds(ctx, f) {
            Ok(b) => b,
            Err(e) => {
                out.notes.push(e.message);
                Book::default()
            }
        };
        for coin in coins.keys() {
            for q in quotes.iter().filter(|q| *q != coin) {
                let pair = super::discover::sorted(*coin, *q);
                let mut found: Vec<_> = book.pairs(pair).chain(seed.pairs(pair)).collect();
                found.sort_unstable();
                found.dedup();
                for (h, first) in found {
                    let id = aqua::edge(h, pair, first);
                    for item in stable_list::candidates(f.id, id, &[pair.0, pair.1], coins, &[*q]) {
                        if item.base == *coin {
                            out.found.entry(*coin).or_default().push(item);
                        }
                    }
                }
            }
        }
    }
    out
}
