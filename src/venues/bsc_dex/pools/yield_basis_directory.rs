//! Enumerate factory markets on a canonical head. Multiple LT generations may reference the same Curve pool.
use super::{
    balancer_v1::{data, views},
    families::{self, Family},
    family_variant::Variant,
    stable_list::{self, Extra},
    verify::{Candidate, Verdict},
    yield_basis::{self as y, IYieldBasis},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::call},
};
use alloy_primitives::{Address, B256, U256};
use std::collections::{HashMap, HashSet};
async fn family(
    ctx: &Ctx,
    f: &Family,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    extra: &mut Extra,
) -> GatewayResult<()> {
    let at = super::nami::block(ctx, "latest").await?;
    let factory = y::factory(ctx, f, &at).await?;
    let r = views(
        ctx,
        &[
            data(factory, IYieldBasis::market_countCall {}),
            data(factory, IYieldBasis::STABLECOINCall {}),
        ],
        &at,
    )
    .await?;
    let count = u64::try_from(call::decode::<IYieldBasis::market_countCall>(&r[0])?)
        .map_err(|_| y::bad(ctx))?;
    let stable = call::decode::<IYieldBasis::STABLECOINCall>(&r[1])?;
    let page = super::load_batch::batch(ctx)?;
    let mut seen = HashSet::new();
    for begin in (0..count).step_by(page) {
        let calls = (begin..count.min(begin.saturating_add(page as u64)))
            .map(|idx| {
                data(
                    factory,
                    IYieldBasis::marketsCall {
                        index: U256::from(idx),
                    },
                )
            })
            .collect::<Vec<_>>();
        let rows = views(ctx, &calls, &at).await?;
        extra.calls += calls.len();
        for (n, row) in rows.iter().enumerate() {
            let raw = call::decode::<IYieldBasis::marketsCall>(row)?;
            for role in 0..3 {
                let Some(m) = y::from_row(begin + n as u64, role, stable, &raw) else {
                    continue;
                };
                if !seen.insert((role, m.venue)) {
                    continue;
                }
                for item in stable_list::candidates(f.id, m.id(), &m.coins(), coins, quotes) {
                    extra.found.entry(item.base).or_default().push(item);
                }
            }
        }
    }
    extra.calls += 2;
    Ok(())
}
pub(super) async fn scan(ctx: &Ctx, coins: &HashMap<Address, String>, quotes: &[Address]) -> Extra {
    let mut extra = Extra::default();
    for f in families::all(ctx) {
        if f.variant == Variant::YieldBasis
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
    let result: GatewayResult<PoolRef> = async {
        let id = c.pool.parse::<B256>().map_err(|_| y::bad(ctx))?;
        let (index, role, venue) = y::key(id).ok_or_else(|| y::bad(ctx))?;
        let at = super::nami::block(ctx, "latest").await?;
        let m = y::market(ctx, c.family, index, role, &at).await?;
        if venue != m.venue
            || !m.coins().contains(&c.coin)
            || !m.coins().contains(&c.quote)
            || c.coin == c.quote
        {
            return Err(y::bad(ctx));
        }
        Ok(PoolRef::Singleton {
            manager: m.cryptopool,
            id,
        })
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
) -> GatewayResult<Vec<Candidate>> {
    use alloy::sol_types::SolEvent;
    let event =
        IYieldBasis::MarketParameters::decode_log_data(&log.inner.data).map_err(|_| y::bad(ctx))?;
    let idx = u64::try_from(event.idx).map_err(|_| y::bad(ctx))?;
    let at = serde_json::json!({"blockHash":log.block_hash.ok_or_else(||y::bad(ctx))?,"requireCanonical":true});
    let factory = y::factory(ctx, f, &at).await?;
    if factory != log.address() {
        return Err(y::bad(ctx));
    }
    let rows = views(
        ctx,
        &[
            data(factory, IYieldBasis::marketsCall { index: event.idx }),
            data(factory, IYieldBasis::STABLECOINCall {}),
        ],
        &at,
    )
    .await?;
    let raw = call::decode::<IYieldBasis::marketsCall>(&rows[0])?;
    let stable = call::decode::<IYieldBasis::STABLECOINCall>(&rows[1])?;
    if raw.amm != event.amm
        || raw.asset != event.asset_token
        || raw.cryptopool != event.cryptopool
        || raw.lt != event.lt
        || raw.oracle != event.oracle
        || raw.virtual_pool != event.virtual_pool
    {
        return Err(y::bad(ctx));
    }
    let mut out = vec![];
    for role in 0..3 {
        let Some(m) = y::from_row(idx, role, stable, &raw) else {
            continue;
        };
        let pair = m.coins();
        let Some(coin) = pair.iter().position(|&t| t == quote).map(|i| pair[1 - i]) else {
            continue;
        };
        if role == 0 {
            // Historical LT generations share Curve liquidity. Emit the lowest factory index.
            let mut old = false;
            let page = super::load_batch::batch(ctx)?;
            for begin in (0..idx).step_by(page) {
                let prior = (begin..idx.min(begin.saturating_add(page as u64)))
                    .map(|i| {
                        data(
                            factory,
                            IYieldBasis::marketsCall {
                                index: U256::from(i),
                            },
                        )
                    })
                    .collect::<Vec<_>>();
                for r in views(ctx, &prior, &at).await? {
                    if call::decode::<IYieldBasis::marketsCall>(&r)?.cryptopool == m.cryptopool {
                        old = true;
                        break;
                    }
                }
                if old {
                    break;
                }
            }
            if old {
                continue;
            }
        }
        out.push(Candidate {
            family: f,
            coin,
            quote,
            pool: format!("{:#x}", m.id()),
        });
    }
    Ok(out)
}
