//! Полные котировки официальных контрактов и независимые лестницы по сторонам DLMM.
use super::{
    families::Family,
    family_variant::Variant,
    ladder_counter, load_metric, nami_batch as batch,
    registry::{Entry, Registry},
    umbrae::IMarket,
};
use crate::{
    core::types::{LadderPoint, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::rpc, tokens::Tokens},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

pub(crate) fn request(
    ctx: &Ctx,
    e: &Entry,
    token: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let pair = e.pool.reference.address();
    let forward = token == e.pool.token0;
    if !forward && token != e.pool.token1 {
        return Err(ctx.err(ErrorCode::NoRoute, "Umbrae: входной актив вне пары"));
    }
    Ok(if e.family.variant == Variant::UmbraeDlmm {
        (
            ctx.dex_addr("dex.quoter", e.family.id)?,
            IMarket::quoteSwapDetailedCall {
                pair,
                forward,
                amount,
            }
            .abi_encode()
            .into(),
        )
    } else {
        (
            pair,
            IMarket::getAmountOutCall {
                amount,
                xToY: forward,
            }
            .abi_encode()
            .into(),
        )
    })
}

pub(crate) fn payout(variant: Variant, raw: &[u8], input: U256) -> Option<U256> {
    if variant == Variant::UmbraeDamm {
        return IMarket::getAmountOutCall::abi_decode_returns(raw)
            .ok()
            .filter(|o| !o.is_zero());
    }
    let r = IMarket::quoteSwapDetailedCall::abi_decode_returns(raw).ok()?;
    (!input.is_zero() && !r.capped && r.used == input && !r.out.is_zero()).then_some(r.out)
}

async fn pass(
    ctx: &Ctx,
    viewer: Address,
    pair: Address,
    forward: bool,
    sizes: &[U256],
    at: &Value,
) -> GatewayResult<Vec<LadderPoint>> {
    if sizes.is_empty() {
        return Ok(Vec::new());
    }
    let calls: Vec<_> = sizes
        .iter()
        .map(|&amount| {
            batch::view(
                at,
                viewer,
                IMarket::quoteSwapDetailedCall {
                    pair,
                    forward,
                    amount,
                },
            )
        })
        .collect();
    let mut points = Vec::new();
    for (&amount_in, reply) in sizes.iter().zip(rpc::batch(ctx, &calls).await?) {
        let raw = match reply {
            Ok(v) => v,
            Err(e) if e.code == ErrorCode::SwapReverted => continue,
            Err(e) => return Err(e),
        };
        let bytes: Bytes = serde_json::from_str(raw.get())
            .map_err(|err| ctx.err(ErrorCode::BadResponse, err.to_string()))?;
        let r = batch::decode(
            &bytes,
            IMarket::quoteSwapDetailedCall {
                pair,
                forward,
                amount: amount_in,
            },
        )?;
        if !r.capped && r.used == amount_in && !r.out.is_zero() {
            points.push(LadderPoint {
                amount_in,
                amount_out: r.out,
            });
        }
    }
    Ok(points)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn ladder(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    f: &Family,
    pool: Address,
    pair: [Address; 2],
    counter: Address,
    at: &Value,
) -> GatewayResult<PoolState> {
    let coin = if pair[0] == counter { pair[1] } else { pair[0] };
    let viewer = ctx.dex_addr("dex.quoter", f.id)?;
    let (buys, mut sells) = tokio::try_join!(
        ladder_counter::amounts(ctx, tokens, reg, f.id, counter),
        ladder_counter::amounts(ctx, tokens, reg, f.id, coin)
    )?;
    let bought = pass(ctx, viewer, pool, counter == pair[0], &buys, at).await?;
    sells.extend(bought.iter().map(|p| p.amount_out));
    sells.sort_unstable();
    sells.dedup();
    let sold = pass(ctx, viewer, pool, coin == pair[0], &sells, at).await?;
    Ok(load_metric::state(
        (pair[0], pair[1]),
        coin,
        pair[0],
        (!bought.is_empty() || !sold.is_empty()).then_some(0),
        bought,
        sold,
    ))
}
