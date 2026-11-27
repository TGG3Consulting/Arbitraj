//! Сверка программы SwapVM и точного режима оплаты на одном каноническом блоке.
use super::{
    aqua_wallet::{IGuard, Proof, Terms},
    chain::Step,
    plan::{DirectCall, Plan},
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::{call, rpc},
        pools::{aqua, aqua_directory, aqua_ladder, dagrow, exact_wallet_tokens, registry::Entry},
        quote::{
            v2_wallet_sim as sim,
            white_state::{self, Head},
        },
        wallet::approve,
    },
};
use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::{Value, json};
use std::sync::Arc;

pub(in crate::venues::bsc_dex) fn terms(
    ctx: &Ctx,
    e: &Entry,
    target: Address,
) -> GatewayResult<Terms> {
    let t = aqua_directory::identity(
        ctx,
        e.family,
        &e.pool.reference,
        (e.pool.token0, e.pool.token1),
    )?;
    let engine = ctx.dex_addr("dex.quoter", e.family.id)?;
    if target != engine && ctx.dex_addr("dex.router", e.family.id)? != target {
        return Err(aqua::bad(ctx));
    }
    Ok(Terms {
        chain: ctx.chain_id().ok_or_else(|| aqua::bad(ctx))?,
        target,
        engine,
        maker: t.program.maker,
        hash: t.hash,
        raw: t.program.abi_encode().into(),
        first: t.first,
    })
}
pub(in crate::venues::bsc_dex) async fn checked(
    ctx: &Ctx,
    e: &Entry,
    target: Address,
    at: &Value,
) -> GatewayResult<Terms> {
    let (roles, _) = aqua_directory::checked(
        ctx,
        e.family,
        &e.pool.reference,
        (e.pool.token0, e.pool.token1),
        at,
    )
    .await?;
    if target == roles.engine {
        exact_wallet_tokens::check(ctx, e.family.id, e.pool.token0, e.pool.token1, at).await?;
    } else if aqua_ladder::module(ctx, e.family, roles, at).await? != target {
        return Err(aqua::bad(ctx));
    }
    terms(ctx, e, target)
}
pub(in crate::venues::bsc_dex) fn graph(ctx: &Ctx, e: &Entry) -> Option<(B256, Address)> {
    let target = ctx.dex_addr("dex.router", e.family.id).ok()?;
    let t = terms(ctx, e, target).ok()?;
    (t.target != t.engine).then(|| (keccak256((t.hash, t.first).abi_encode_params()), target))
}
pub(in crate::venues::bsc_dex) async fn cached(
    ctx: &Ctx,
    e: &Entry,
    h: Head,
    v5: Address,
) -> GatewayResult<()> {
    let (key, module) = graph(ctx, e).ok_or_else(|| aqua::bad(ctx))?;
    let t = checked(ctx, e, module, &h.selector()).await?;
    let call = |c: Bytes| {
        rpc::call::<Bytes>(
            ctx,
            "eth_call",
            json!([{"to":module,"data":c},h.selector()]),
        )
    };
    let (id, accepted, code) = tokio::try_join!(
        call(IGuard::orderForKeyCall { key }.abi_encode().into()),
        call(IGuard::v5CodeCall {}.abi_encode().into()),
        rpc::call::<Bytes>(ctx, "eth_getCode", json!([v5, h.selector()]))
    )?;
    if code.is_empty()
        || call::decode::<IGuard::orderForKeyCall>(&id)? != t.hash
        || call::decode::<IGuard::v5CodeCall>(&accepted)? != keccak256(code)
    {
        return Err(aqua::bad(ctx));
    }
    Ok(())
}
pub(super) async fn plan(
    ctx: &Ctx,
    s: &Step,
    payer: Address,
    height: u64,
    target: Option<Address>,
) -> GatewayResult<Plan> {
    let g = ctx.settings.generation();
    if !s.from_wallet || !s.to_wallet {
        return Err(aqua::bad(ctx));
    }
    let h = white_state::head(ctx, &format!("{height:#x}")).await?;
    let t = checked(
        ctx,
        &s.entry,
        target.ok_or_else(|| aqua::bad(ctx))?,
        &h.selector(),
    )
    .await?;
    let target = t.target;
    let o = &s.order;
    let order = sim::Order {
        payer,
        target,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: sim::Wire::Aqua(Arc::new(t.clone())),
    };
    let deadline = U256::from(
        h.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| aqua::bad(ctx))?,
    );
    let proof = t.proof(&order, o.min_out, deadline);
    sim::measure(ctx, h, order, true).await?;
    let allowance = dagrow::allowance(ctx, o.token_in, payer, target, &h.selector()).await?;
    let approvals = approve::fork_exact(ctx, o.token_in, target, Some(allowance), o.amount_in)?;
    if g != ctx.settings.generation() {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Aqua: настройки изменились перед оплатой",
        ));
    }
    let mut plan = Plan::new(
        ctx,
        (approvals, target, payer),
        o.token_out,
        vec![],
        o.min_out,
        deadline,
    )?;
    plan.set_direct_market(DirectCall::Aqua(Arc::new(proof)));
    Ok(plan)
}
pub(super) fn probe(
    ctx: &Ctx,
    e: &Entry,
    o: &super::plan::Order,
    payer: Address,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, Bytes)> {
    let target = ctx
        .dex_addr("dex.router", e.family.id)
        .or_else(|_| ctx.dex_addr("dex.quoter", e.family.id))?;
    probe_at(ctx, e, o, payer, now, target)
}
pub(in crate::venues::bsc_dex) fn probe_at(
    ctx: &Ctx,
    e: &Entry,
    o: &super::plan::Order,
    payer: Address,
    now: u64,
    target: Address,
) -> GatewayResult<(Vec<approve::Approval>, Address, Bytes)> {
    let proof = Proof {
        terms: terms(ctx, e, target)?,
        payer,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        deadline: U256::from(
            now.checked_add(ctx.int("dex.deadline_s")?)
                .ok_or_else(|| aqua::bad(ctx))?,
        ),
    };
    Ok((
        approve::fork_exact(ctx, o.token_in, target, None, o.amount_in)?,
        target,
        proof.data(),
    ))
}
