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

tokio::task_local! {pub(super) static NATIVE:bool;}
pub(in crate::venues::bsc_dex) fn probe(
    ctx: &Ctx,
    e: &super::super::pools::registry::Entry,
    o: &super::plan::Order,
    now: u64,
) -> GatewayResult<(
    Vec<super::super::wallet::approve::Approval>,
    Address,
    alloy_primitives::Bytes,
)> {
    let key = ekubo_v2::order(e, o.token_in, o.amount_in).ok_or_else(|| {
        ctx.err(
            ErrorCode::NoRoute,
            "Ключ Ekubo V2 не совпал с токенами пробы",
        )
    })?;
    let router = ctx.dex_addr("dex.quoter", e.family.id)?;
    let aps = if key.value().is_zero() {
        super::super::wallet::approve::fork_all(ctx, o.token_in, router, o.amount_in)?
    } else {
        Vec::new()
    };
    Ok((
        aps,
        router,
        key.calldata(
            o.min_out,
            U256::from(now.saturating_add(ctx.int("dex.deadline_s")?)),
        ),
    ))
}
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
            "Ekubo V2: требуется один подтверждённый рынок с правильной валютой кошелька",
        )
    };
    let [s] = steps else {
        return Err(bad());
    };
    if !s.from_wallet
        || !s.to_wallet
        || !matches!(&s.entry.pool.state,PoolState::Ekubo(d) if d.native.is_some()==NATIVE.try_with(|v|*v).unwrap_or(false))
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
    let wrapped = match &s.entry.pool.state {
        PoolState::Ekubo(d) => d.native,
        _ => return Err(bad()),
    };
    let measure = v2_wallet_sim::Order {
        payer: wallet,
        target: router,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: v2_wallet_sim::Wire::EkuboV2(key, wrapped),
    };
    if !all {
        Box::pin(v2_wallet_sim::measure(ctx, h, measure, true)).await?;
    }
    let approvals = if !key.value().is_zero() {
        Vec::new()
    } else if all {
        super::super::wallet::approve::fork_all(ctx, o.token_in, router, o.amount_in)?
    } else {
        super::super::wallet::approve::fork_missing(ctx, o.token_in, wallet, router, o.amount_in)
            .await?
    };
    if ctx.settings.generation() != generation {
        return Err(bad());
    }
    let mut plan = Plan::new(
        ctx,
        (approvals, router, wallet),
        o.token_out,
        vec![],
        o.min_out,
        U256::from(now.saturating_add(ctx.int("dex.deadline_s")?)),
    )?;
    plan.set_direct_market(DirectCall::EkuboV2 {
        key,
        core: s.entry.pool.reference.address(),
        wrapped,
    });
    Ok(plan)
}
