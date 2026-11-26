//! Готовит одноточечный обмен ERC-20 кошелька через канонический DeltaSwapRouter02.
use super::super::{
    ctx::Ctx,
    pools::deltaswap,
    quote::{pool_quote, white_state},
};
use super::{chain::Step, plan::Plan, white_direct::build};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy_primitives::Address;

pub(super) async fn plan(
    ctx: &Ctx,
    steps: &[Step],
    wallet: Address,
    now: u64,
    all: bool,
) -> GatewayResult<Plan> {
    let [step] = steps else {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "DeltaSwap Router02: требуется ровно один рынок",
        ));
    };
    let generation = ctx.settings.generation();
    if !step.from_wallet || !step.to_wallet {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "DeltaSwap Router02: обмен должен начинаться и завершаться у кошелька",
        ));
    }
    let mut entry = step.entry.clone();
    let original = deltaswap::router(ctx, &entry)?;
    let head = white_state::head(ctx, "latest").await?;
    deltaswap::refresh(ctx, &mut entry, head).await?;
    let router = deltaswap::router(ctx, &entry)?;
    let order = &step.order;
    let pair = (entry.pool.token0, entry.pool.token1);
    if (order.token_in, order.token_out) != pair && (order.token_out, order.token_in) != pair {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "DeltaSwap: токены отправки не совпали с рынком",
        ));
    }
    let quote = pool_quote::swap(ctx, &entry, order.token_in, true, order.amount_in)?;
    if router != original
        || !quote.complete
        || quote.amount_out < order.min_out
        || generation != ctx.settings.generation()
    {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "DeltaSwap: изменился роутер, настройки или достижимый выход",
        ));
    }
    build(ctx, step, wallet, router, now, all, generation).await
}
