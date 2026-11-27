//! Котировка с реальным хуком на одном хеше. Лесенки покупки и продажи независимы.
use super::{
    families::Family, key::PoolKey, ladder_counter, launch_keys::invalid, launch_market::Market,
    load::Loaded, load_metric, nami_batch as batch, registry::Registry,
};
use crate::{
    core::types::LadderPoint,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::{abi_dex::IV4Quoter, rpc},
        tokens::Tokens,
    },
};
use alloy_primitives::{Address, B256, Bytes, U256, aliases::U24};
use serde_json::Value;

async fn pass(
    ctx: &Ctx,
    viewer: Address,
    key: &PoolKey,
    token: Address,
    sizes: &[U256],
    at: &Value,
    witness: Option<&(crate::venues::bsc_dex::quote::white_state::Head, Bytes)>,
) -> GatewayResult<Vec<LadderPoint>> {
    let PoolKey::Uniswap(k) = key else {
        return Err(invalid(ctx));
    };
    let pool = IV4Quoter::PoolKey {
        currency0: k.currency0,
        currency1: k.currency1,
        fee: U24::from(k.fee),
        tickSpacing: k.tick_spacing,
        hooks: k.hooks,
    };
    let quotes: Vec<_> = sizes
        .iter()
        .filter_map(|size| {
            let exact = u128::try_from(*size).ok().filter(|n| *n != 0)?;
            let params = IV4Quoter::QuoteExactSingleParams {
                poolKey: pool.clone(),
                zeroForOne: token == k.currency0,
                exactAmount: exact,
                hookData: witness.map(|(_, b)| b.clone()).unwrap_or_default(),
            };
            Some((*size, IV4Quoter::quoteExactInputSingleCall { params }))
        })
        .collect();
    if quotes.is_empty() {
        return Ok(Vec::new());
    }
    if let Some((head, _)) = witness {
        return super::angstrom_ladder::quotes(ctx, viewer, *head, &quotes).await;
    }
    let calls: Vec<_> = quotes
        .iter()
        .map(|(_, c)| batch::view(at, viewer, c.clone()))
        .collect();
    let mut out = Vec::new();
    for ((amount_in, call), response) in quotes.into_iter().zip(rpc::batch(ctx, &calls).await?) {
        let raw = match response {
            Ok(raw) => raw,
            Err(e) if e.code == ErrorCode::SwapReverted => continue,
            Err(e) => return Err(e),
        };
        let b: Bytes = serde_json::from_str(raw.get())
            .map_err(|e| ctx.err(ErrorCode::BadResponse, e.to_string()))?;
        let amount_out = batch::decode(&b, call)?.amountOut;
        if !amount_out.is_zero() {
            out.push(LadderPoint {
                amount_in,
                amount_out,
            });
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    f: &Family,
    manager: Address,
    id: B256,
    pair: (Address, Address),
    tag: &str,
) -> GatewayResult<Loaded> {
    let generation = ctx.settings.generation();
    let at = super::nami::block(ctx, tag).await?;
    let Market {
        key,
        asset,
        buys,
        sells,
    } = super::launch_market::market(ctx, f, manager, id, pair, &at).await?;
    let cash = if asset == pair.0 { pair.1 } else { pair.0 };
    // Нативный ETH и WETH используют одинаковые decimals для размеров, отдельные валюты для расчёта/оплаты.
    let counter = if cash.is_zero() {
        ctx.addr("dex.native_wrapped")?
    } else {
        cash
    };
    let quoter = ctx.dex_addr("dex.quoter", f.id)?;
    let (sizes, mut sellsizes) = tokio::try_join!(
        ladder_counter::amounts(ctx, tokens, reg, f.id, counter),
        ladder_counter::amounts(ctx, tokens, reg, f.id, asset)
    )?;
    let witness = if f.variant == super::family_variant::Variant::Angstrom {
        super::angstrom_ladder::witness(ctx, f.id, &at).await?
    } else {
        None
    };
    let bought = if buys {
        let input = if f.variant == super::family_variant::Variant::LaunchO1Wrapped {
            Address::ZERO
        } else {
            cash
        };
        pass(ctx, quoter, &key, input, &sizes, &at, witness.as_ref()).await?
    } else {
        Vec::new()
    };
    sellsizes.extend(bought.iter().map(|p| p.amount_out));
    sellsizes.sort_unstable();
    sellsizes.dedup();
    let sold = if sells {
        pass(ctx, quoter, &key, asset, &sellsizes, &at, witness.as_ref()).await?
    } else {
        Vec::new()
    };
    if generation != ctx.settings.generation() {
        return Err(invalid(ctx));
    }
    let feasible = (!bought.is_empty() || !sold.is_empty()).then_some(0);
    Ok(Loaded {
        state: load_metric::state(pair, asset, pair.0, feasible, bought, sold),
        ticks: None,
        bins: None,
        fee_pips: None,
    })
}
