//! Лестницы по двум сторонам из rateByPath исходного Network при одном состоянии конвертера.
use super::{
    bancor_legacy::{self, ILegacy},
    families::Family,
    ladder_counter, nami_batch as batch,
    registry::Registry,
};
use crate::{
    core::types::{LadderPoint, PoolState, bancor_legacy::BancorLegacyState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::rpc, tokens::Tokens},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

async fn levels(
    ctx: &Ctx,
    s: &BancorLegacyState,
    side: usize,
    sizes: &[U256],
    at: &Value,
) -> GatewayResult<Vec<LadderPoint>> {
    if sizes.is_empty() {
        return Ok(vec![]);
    }
    let path = s
        .path(s.tokens[side])
        .ok_or_else(|| bancor_legacy::bad(ctx))?;
    let asks = sizes
        .iter()
        .map(|&amount| {
            batch::view(
                at,
                s.network,
                ILegacy::rateByPathCall {
                    path: path.clone(),
                    amount,
                },
            )
        })
        .collect::<Vec<_>>();
    let rows = rpc::batch(ctx, &asks).await?;
    if rows.len() != sizes.len() {
        return Err(bancor_legacy::bad(ctx));
    }
    let mut result = Vec::new();
    for (&amount_in, row) in sizes.iter().zip(rows) {
        let raw = match row {
            Ok(row) => row,
            Err(e) if e.code == ErrorCode::SwapReverted => continue,
            Err(e) => return Err(e),
        };
        let raw: Bytes = serde_json::from_str(raw.get()).map_err(|_| bancor_legacy::bad(ctx))?;
        if raw.len() != 32 {
            return Err(bancor_legacy::bad(ctx));
        }
        let amount_out = ILegacy::rateByPathCall::abi_decode_returns_validate(&raw)
            .map_err(|_| bancor_legacy::bad(ctx))?;
        if !amount_in.is_zero() && !amount_out.is_zero() {
            result.push(LadderPoint {
                amount_in,
                amount_out,
            });
        }
    }
    Ok(result)
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    f: &Family,
    pool: &crate::core::types::PoolRef,
    pair: (Address, Address),
    counter: Address,
    at: &Value,
) -> GatewayResult<PoolState> {
    let mut state = super::bancor_legacy_bridge::bound(ctx, f, pool, pair, at).await?;
    let i = usize::from(counter != state.tokens[0]);
    if counter != state.tokens[i] {
        return Err(bancor_legacy::bad(ctx));
    }
    let (mut inputs, mut reverse) = tokio::try_join!(
        ladder_counter::amounts(ctx, tokens, reg, f.id, counter),
        ladder_counter::amounts(ctx, tokens, reg, f.id, state.tokens[1 - i])
    )?;
    inputs.sort_unstable();
    inputs.dedup();
    state.levels[i] = levels(ctx, &state, i, &inputs, at).await?;
    reverse.extend(state.levels[i].iter().map(|point| point.amount_out));
    reverse.sort_unstable();
    reverse.dedup();
    state.levels[1 - i] = levels(ctx, &state, 1 - i, &reverse, at).await?;
    Ok(PoolState::BancorLegacy(Box::new(state)))
}
