//! Волатильный пул Wombat хранит меняющуюся цену трёх активов. Каждый уровень берётся
//! из quotePotentialSwap на одном каноническом блоке, обе стороны измеряются независимо.
use super::{
    abi_wombat::{IWombatPool as P, implementation_slot},
    families::Family,
    ladder_counter, load_metric, nami_batch as batch,
    registry::Registry,
};
use crate::{
    core::types::{LadderPoint, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::rpc, tokens::Tokens},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, I256, U256, keccak256};
use serde_json::{Value, json};

pub(in crate::venues::bsc_dex) async fn market(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<bool> {
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Wombat: состав пула, реализация или байткод не подтверждены",
        )
    };
    let generation = ctx.settings.generation();
    let implementation = ctx.dex_addr("dex.pool_implementation", f.id)?;
    let hashes = ctx.dex_list("dex.proxy_runtime_hashes", f.id)?;
    let [proxy, logic] = hashes.as_slice() else {
        return Err(bad());
    };
    let proxy = proxy.parse::<B256>().map_err(|_| bad())?;
    let logic = logic.parse::<B256>().map_err(|_| bad())?;
    if pool.is_zero()
        || implementation.is_zero()
        || pool == implementation
        || proxy == logic
        || pair.0 == pair.1
        || !ctx
            .dex_list("dex.stable_pools", f.id)?
            .iter()
            .any(|a| a.parse().ok() == Some(pool))
    {
        return Err(bad());
    }
    let listing = P::getTokensCall {};
    let pause = P::pausedCall {};
    let [slot, proxy_code, impl_code, tokens, paused] = batch::read(
        ctx,
        [
            ("eth_getStorageAt", json!([pool, implementation_slot(), at])),
            ("eth_getCode", json!([pool, at])),
            ("eth_getCode", json!([implementation, at])),
            batch::view(at, pool, listing.clone()),
            batch::view(at, pool, pause.clone()),
        ],
    )
    .await?;
    let tokens = batch::decode(&tokens, listing)?;
    if slot.as_ref() != implementation.into_word().as_slice()
        || proxy_code.is_empty()
        || impl_code.is_empty()
        || keccak256(&proxy_code) != proxy
        || keccak256(&impl_code) != logic
        || !tokens.contains(&pair.0)
        || !tokens.contains(&pair.1)
        || generation != ctx.settings.generation()
    {
        return Err(bad());
    }
    Ok(!batch::decode(&paused, pause)?)
}

async fn levels(
    ctx: &Ctx,
    pool: Address,
    pair: (Address, Address),
    sizes: &[U256],
    at: &Value,
) -> GatewayResult<Vec<LadderPoint>> {
    let mut req = Vec::new();
    let mut amounts = Vec::new();
    for &amount in sizes {
        if let Ok(from_amount) = I256::try_from(amount) {
            amounts.push(amount);
            req.push(batch::view(
                at,
                pool,
                P::quotePotentialSwapCall {
                    fromToken: pair.0,
                    toToken: pair.1,
                    fromAmount: from_amount,
                },
            ));
        }
    }
    let mut points = Vec::new();
    if req.is_empty() {
        return Ok(points);
    }
    let rows = rpc::batch(ctx, &req).await?;
    let bad = || {
        ctx.err(
            ErrorCode::BadResponse,
            "Wombat: неполный ответ quotePotentialSwap",
        )
    };
    if rows.len() != amounts.len() {
        return Err(bad());
    }
    for (n, row) in amounts.into_iter().zip(rows) {
        let raw = match row {
            Ok(r) => r,
            Err(e) if e.code == ErrorCode::SwapReverted => continue,
            Err(e) => return Err(e),
        };
        let bytes: Bytes = serde_json::from_str(raw.get()).map_err(|_| bad())?;
        let result = P::quotePotentialSwapCall::abi_decode_returns(&bytes).map_err(|_| bad())?;
        if !result.potentialOutcome.is_zero() {
            points.push(LadderPoint {
                amount_in: n,
                amount_out: result.potentialOutcome,
            });
        }
    }
    Ok(points)
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    counter: Address,
    at: &Value,
) -> GatewayResult<PoolState> {
    let coin = if pair.0 == counter {
        pair.1
    } else if pair.1 == counter {
        pair.0
    } else {
        return Err(ctx.err(ErrorCode::NoRoute, "Wombat: встречной монеты нет в паре"));
    };
    let (bought, sold) = if market(ctx, f, pool, pair, at).await? {
        let (buys, mut sells) = tokio::try_join!(
            ladder_counter::amounts(ctx, tokens, reg, f.id, counter),
            ladder_counter::amounts(ctx, tokens, reg, f.id, coin)
        )?;
        let bought = levels(ctx, pool, (counter, coin), &buys, at).await?;
        sells.extend(bought.iter().map(|level| level.amount_out));
        sells.sort_unstable();
        sells.dedup();
        let sold = levels(ctx, pool, (coin, counter), &sells, at).await?;
        (bought, sold)
    } else {
        (Vec::new(), Vec::new())
    };
    let mut state = load_metric::state(
        pair,
        coin,
        pair.0,
        (!bought.is_empty() || !sold.is_empty()).then_some(0),
        bought,
        sold,
    );
    if let PoolState::OraclePmm(d) = &mut state {
        d.shared_accounts.push(pool);
    }
    Ok(state)
}
