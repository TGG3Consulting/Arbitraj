//! Однопуловый обмен EOA через официальный Fraxswap с проверкой поступления на кошелёк.
//! Штатные стадии разрешений, повторной симуляции, подписи и сверки сохраняются.
use super::super::{ctx::Ctx, quote::white_state, wallet::approve};
use super::{chain::Step, plan::Plan};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy_primitives::{Address, U256};

tokio::task_local! {pub(super) static SCOPE:();}
pub(in crate::venues::bsc_dex) fn selected() -> bool {
    SCOPE.try_with(|_| ()).is_ok()
}

pub(super) async fn plan(
    ctx: &Ctx,
    steps: &[Step],
    wallet: Address,
    now: u64,
    all: bool,
) -> GatewayResult<Plan> {
    let [s] = steps else {
        return Err(ctx.err(ErrorCode::NoRoute, "Fraxswap требует один пул"));
    };
    if !s.to_wallet || !s.from_wallet {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Fraxswap: требуется ERC-20 кошелька для обеих сторон",
        ));
    }
    let generation = ctx.settings.generation();
    let h = white_state::head(ctx, "latest").await?;
    let e = &s.entry;
    if !super::super::quote::frax_pricing::is_frax(e) {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Fraxswap: прямой обмен отключён для этого DEX",
        ));
    }
    let router = super::super::pools::fraxswap::native(ctx, e.family, &h.selector()).await?;
    super::super::pools::fraxswap::validate(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.pool.token0, e.pool.token1),
        &h.selector(),
    )
    .await?;
    super::white_direct::build(ctx, s, wallet, router, now, all, generation).await
}

/// Та же функция роутера в пробе двух направлений, до допуска пула к котировкам.
pub(in crate::venues::bsc_dex) async fn probe(
    ctx: &Ctx,
    e: &super::super::pools::registry::Entry,
    o: &super::plan::Order,
    wallet: Address,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, alloy_primitives::Bytes)> {
    let h = white_state::head(ctx, "latest").await?;
    let router = super::super::pools::fraxswap::native(ctx, e.family, &h.selector()).await?;
    super::super::pools::fraxswap::validate(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.pool.token0, e.pool.token1),
        &h.selector(),
    )
    .await?;
    let data = super::fork_swap::swap_data(
        o.amount_in,
        o.min_out,
        &[o.token_in, o.token_out],
        wallet,
        U256::from(now + ctx.int("dex.deadline_s")?),
    );
    Ok((
        approve::fork_all(ctx, o.token_in, router, o.amount_in)?,
        router,
        data,
    ))
}
