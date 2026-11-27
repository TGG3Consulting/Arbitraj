//! Подготовка кошелькового поручения o1 на одной высоте.
use super::super::{
    ctx::Ctx,
    quote::{v2_wallet_sim, white_state},
    wallet::approve,
};
use super::{
    chain::Step,
    o1_wallet::{Proof, at, key, missing},
    plan::{DirectCall, Plan},
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy_primitives::{Address, U256};
pub(super) async fn plan(
    ctx: &Ctx,
    step: &Step,
    payer: Address,
    height: u64,
    selected: Option<Address>,
) -> GatewayResult<Plan> {
    let e = &step.entry;
    let o = &step.order;
    if !step.from_wallet || !step.to_wallet {
        return Err(missing(ctx));
    }
    let generation = ctx.settings.generation();
    let module = ctx.dex_addr("dex.router", e.family.id)?;
    if selected != Some(module) {
        return Err(missing(ctx));
    }
    let id = key(ctx, e)?;
    let h = white_state::head(ctx, &format!("{height:#x}")).await?;
    at(ctx, e, &h.selector()).await?;
    let deadline = U256::from(
        h.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| missing(ctx))?,
    );
    let p = Proof {
        id,
        module,
        payer,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        deadline,
    };
    let order = v2_wallet_sim::Order {
        payer,
        target: module,
        input: p.input,
        output: p.output,
        amount: p.amount,
        minimum: p.minimum,
        wire: v2_wallet_sim::Wire::O1(id),
    };
    v2_wallet_sim::measure(ctx, h, order, true).await?;
    let approvals = approve::fork_missing(ctx, p.input, payer, module, p.amount).await?;
    if generation != ctx.settings.generation() {
        return Err(ctx.err(ErrorCode::StaleData, "Настройки o1 изменились"));
    }
    let mut plan = Plan::new(
        ctx,
        (approvals, module, payer),
        p.output,
        vec![],
        p.minimum,
        p.deadline,
    )?;
    plan.set_direct_market(DirectCall::O1(std::sync::Arc::new(p)));
    Ok(plan)
}
