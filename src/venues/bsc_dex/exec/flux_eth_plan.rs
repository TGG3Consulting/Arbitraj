//! Выбор и оплата официального Omni Flux Router. Один канонический блок связывает
//! код получателя разрешения, адреса менеджера/Vault, исходный PoolId и исполнимую сумму.
use super::super::{
    ctx::Ctx,
    node::abi::IERC20,
    quote::{
        v2_wallet_sim::{self, Order, Wire},
        white_state::{self, Head},
    },
    wallet::approve::{self, Approval},
};
use super::{
    chain::Step,
    flux_eth::Market,
    plan::{DirectCall, Plan},
};
use crate::gateway::errors::GatewayResult;
use alloy_primitives::{Address, U256};
#[path = "flux_eth_roles.rs"]
mod roles;
pub(in crate::venues::bsc_dex) use roles::checked;
use roles::{bad, view};
pub(in crate::venues::bsc_dex) async fn approvals(
    ctx: &Ctx,
    h: Head,
    m: Market,
    o: &Order,
) -> GatewayResult<Vec<Approval>> {
    if o.target != m.router {
        return Err(bad(ctx));
    }
    if o.input == m.wrapped && o.output == m.token {
        return Ok(Vec::new());
    }
    if (o.input, o.output) != (m.token, m.wrapped) {
        return Err(bad(ctx));
    }
    let at = h.selector();
    if m.module {
        let current = view(
            ctx,
            &at,
            m.token,
            IERC20::allowanceCall {
                owner: o.payer,
                spender: m.router,
            },
        )
        .await?;
        return approve::fork_known(ctx, m.token, m.router, current, o.amount);
    }
    approve::at_block(ctx,m.token,o.payer,m.permit,m.router,o.amount,h).await
}
pub(super) async fn plan(
    ctx: &Ctx,
    s: &Step,
    payer: Address,
    height: u64,
    selected: Option<Address>,
) -> GatewayResult<Plan> {
    if !s.from_wallet || !s.to_wallet {
        return Err(bad(ctx));
    }
    let generation = ctx.settings.generation();
    let h = white_state::head(ctx, &format!("{height:#x}")).await?;
    let public = ctx.dex_addr("dex.universal_router", s.entry.family.id)?;
    let m = checked(ctx, &s.entry, &h.selector(), selected != Some(public)).await?;
    if selected != Some(m.router) {
        return Err(bad(ctx));
    }
    let x = &s.order;
    let o = Order {
        payer,
        target: m.router,
        input: x.token_in,
        output: x.token_out,
        amount: x.amount_in,
        minimum: x.min_out,
        wire: Wire::FluxEth(m),
    };
    let deadline = h
        .timestamp
        .to::<u64>()
        .checked_add(ctx.int("dex.deadline_s")?)
        .ok_or_else(|| bad(ctx))?;
    let proof = m.proof(&o, o.minimum, U256::from(deadline));
    let auth = approvals(ctx, h, m, &o).await?;
    v2_wallet_sim::measure(ctx, h, o, true).await?;
    if generation != ctx.settings.generation() {
        return Err(bad(ctx));
    }
    let mut p = Plan::new(
        ctx,
        (auth, m.router, payer),
        proof.output,
        Vec::new(),
        proof.minimum,
        proof.deadline,
    )?;
    p.set_direct_market(DirectCall::FluxEth(std::sync::Arc::new(proof)));
    Ok(p)
}
