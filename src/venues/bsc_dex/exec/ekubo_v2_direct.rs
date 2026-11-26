//! Native Router of Ethereum Ekubo V2. Enforces a single market, deadline and wallet-level minimum.
use super::super::{
    ctx::Ctx,
    pools::ekubo_v2,
    quote::{v2_wallet_sim, white_state},
};
use super::{
    chain::Step,
    plan::{DirectCall, Plan},
};
use crate::{
    core::types::PoolState,
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy_primitives::{Address, U256};

pub(super) async fn plan(
    ctx: &Ctx,
    steps: &[Step],
    wallet: Address,
    now: u64,
    all: bool,
) -> GatewayResult<Plan> {
    let bad = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Ekubo V2: требуется один подтверждённый ERC-20 рынок кошелька",
        )
    };
    let [s] = steps else {
        return Err(bad());
    };
    if !s.from_wallet
        || !s.to_wallet
        || !matches!(&s.entry.pool.state,PoolState::Ekubo(d) if d.native.is_none())
    {
        return Err(bad());
    }
    let generation = ctx.settings.generation();
    let h = white_state::head(ctx, "latest").await?;
    let router = ekubo_v2::check(
        ctx,
        s.entry.family,
        s.entry.pool.reference.address(),
        &h.selector(),
    )
    .await?;
    let o = &s.order;
    let key = ekubo_v2::order(&s.entry, o.token_in, o.amount_in).ok_or_else(bad)?;
    let measure = v2_wallet_sim::Order {
        payer: wallet,
        target: router,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: v2_wallet_sim::Wire::EkuboV2(key),
    };
    if !all {
        Box::pin(v2_wallet_sim::measure(ctx, h, measure, true)).await?;
    }
    let mut plan = super::white_direct::build(ctx, s, wallet, router, now, all, generation).await?;
    plan.direct_market = Some(DirectCall::EkuboV2(key));
    Ok(plan)
}
