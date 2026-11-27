//! A single pairs() enumerates the immutable id-to-token registry. Every selected pair is then verified with pair().
use super::super::{ctx::Ctx, directory::gt_parse::IndexedPool};
use super::{
    carbon, discover,
    families::{self, Family},
    family_variant::Variant,
    stable_list::Extra,
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256};
use std::collections::HashMap;

async fn registry(
    ctx: &Ctx,
    f: &Family,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
) -> GatewayResult<Vec<IndexedPool>> {
    let at = super::nami::block(ctx, "latest").await?;
    let controller = carbon::controller(ctx, f, &at).await?;
    let [raw] = super::nami_batch::read(
        ctx,
        [super::nami_batch::view(
            &at,
            controller,
            carbon::ICarbon::pairsCall {},
        )],
    )
    .await?;
    let pairs = super::nami_batch::decode(&raw, carbon::ICarbon::pairsCall {})?;
    let wrapped = ctx.addr("dex.native_wrapped").ok();
    let mut found = Vec::new();
    for (i, pair) in pairs.into_iter().enumerate() {
        let Some((s, _)) = carbon::normalized(pair, wrapped) else {
            continue;
        };
        let id = carbon::pair_id(i as u128 + 1);
        for (a, b) in [(s[0], s[1]), (s[1], s[0])] {
            if let Some(symbol) = coins.get(&a)
                && quotes.contains(&b)
            {
                found.push(IndexedPool {
                    dex_id: f.id.into(),
                    pool: format!("{id}"),
                    base: a,
                    quote: b,
                    base_symbol: symbol.clone(),
                    quote_symbol: coins.get(&b).cloned().unwrap_or_else(|| format!("{b:#x}")),
                    reserve_usd: None,
                });
            }
        }
    }
    Ok(found)
}
pub(super) async fn scan(ctx: &Ctx, coins: &HashMap<Address, String>, quotes: &[Address]) -> Extra {
    let mut out = Extra::default();
    for f in families::all(ctx) {
        if f.variant != Variant::Carbon || families::classify(ctx, f.id).is_err() {
            continue;
        }
        out.calls += 1;
        match registry(ctx, f, coins, quotes).await {
            Ok(rows) => {
                for row in rows {
                    out.found.entry(row.base).or_default().push(row);
                }
            }
            Err(e) => {
                out.no_answer += 1;
                out.notes.push(format!("{}: {}", f.id, e.message));
            }
        }
    }
    out
}
pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let Some(id) = c.pool.parse::<B256>().ok().filter(|id| !id.is_zero()) else {
        return Verdict::Rejected("Carbon: нужен ID зарегистрированной пары Controller".into());
    };
    let lookup = async {
        let controller = ctx.dex_addr("dex.factory", c.family.id)?;
        let at = super::nami::block(ctx, "latest").await?;
        carbon::market(
            ctx,
            c.family,
            controller,
            id,
            discover::sorted(c.coin, c.quote),
            &at,
        )
        .await?;
        Ok::<_, crate::gateway::errors::GatewayError>(super::verify_woo::found(
            c,
            PoolRef::Singleton {
                manager: controller,
                id,
            },
        ))
    }
    .await;
    match lookup {
        Ok(found) => Verdict::Taken(Box::new(found)),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
pub(super) fn created(
    ctx: &Ctx,
    f: &'static Family,
    log: &Log,
    quote: Address,
) -> Result<Option<Candidate>, String> {
    let p = carbon::ICarbon::PairCreated::decode_log_data(&log.inner.data)
        .map_err(|e| e.to_string())?;
    let Some((t, _)) =
        carbon::normalized([p.token0, p.token1], ctx.addr("dex.native_wrapped").ok())
    else {
        return Ok(None);
    };
    let coin = if t[0] == quote {
        t[1]
    } else if t[1] == quote {
        t[0]
    } else {
        return Ok(None);
    };
    Ok(Some(Candidate {
        family: f,
        pool: carbon::pair_id(p.pairId).to_string(),
        coin,
        quote,
    }))
}
