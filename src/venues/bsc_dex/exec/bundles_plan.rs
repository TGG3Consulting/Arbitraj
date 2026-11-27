//! Выбор плательщика и повторная проверка Bundles непосредственно перед подписью.
use super::super::{
    ctx::Ctx,
    math::bundles::Book,
    pools::{bundles, dagrow, exact_wallet_tokens, registry::Entry},
    quote::{v2_wallet_sim as sim, white_state},
    wallet::approve,
};
use super::{
    bundles_wallet::Terms,
    chain::Step,
    plan::{DirectCall, Order, Plan},
};
use crate::{
    core::types::PoolState,
    gateway::errors::{ErrorCode, GatewayError, GatewayResult},
};
use alloy_primitives::{Address, Bytes, U256};
pub(in crate::venues::bsc_dex) fn fail(ctx: &Ctx) -> GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Bundles Swap: плательщик, пара, комиссия или минимальная чистая выплата не подтверждены",
    )
}
pub(in crate::venues::bsc_dex) fn terms(
    ctx: &Ctx,
    e: &Entry,
    book: &Book,
    target: Address,
    input: Address,
    amount: U256,
) -> GatewayResult<Terms> {
    let router = ctx.dex_addr("dex.quoter", e.family.id)?;
    let m = &book.market;
    if target.is_zero()
        || (target != router
            && (e.family.executor_kind.is_none()
                || ctx.dex_addr("dex.router", e.family.id)? != target))
    {
        return Err(fail(ctx));
    }
    let i = m
        .tokens
        .iter()
        .position(|&a| a == input)
        .ok_or_else(|| fail(ctx))?;
    let fee_limit = if target == router && m.tokens[0] == book.bun {
        let fill = book
            .exact_in(input, amount, false)
            .ok_or_else(|| fail(ctx))?;
        let tier = U256::from(m.tier);
        if i == 0 {
            amount.checked_mul(tier).ok_or_else(|| fail(ctx))? / (U256::from(1000) + tier)
        } else {
            fill.credit.checked_mul(tier).ok_or_else(|| fail(ctx))? / U256::from(1000)
        }
    } else {
        U256::ZERO
    };
    Ok(Terms {
        pool: m.pool,
        router,
        target,
        bun: book.bun,
        weth: book.weth,
        token0: m.tokens[0],
        tier: m.tier,
        fee_limit,
    })
}
pub(super) async fn plan(
    ctx: &Ctx,
    step: &Step,
    payer: Address,
    height: u64,
    selection: Option<Address>,
) -> GatewayResult<Plan> {
    let generation = ctx.settings.generation();
    let e = &step.entry;
    let o = &step.order;
    let target = selection.ok_or_else(|| fail(ctx))?;
    if !step.from_wallet || !step.to_wallet {
        return Err(fail(ctx));
    }
    let head = white_state::head(ctx, &format!("{height:#x}")).await?;
    let at = head.selector();
    let book = bundles::book(
        ctx,
        e.family,
        e.pool.reference.address(),
        (o.token_in, o.token_out),
        &at,
    )
    .await?;
    let t = terms(ctx, e, &book, target, o.token_in, o.amount_in)?;
    if t.target == t.router {
        exact_wallet_tokens::check(ctx, e.family.id, o.token_in, o.token_out, &at).await?;
    } else {
        bundles::module(ctx, e, target, &at).await?;
    }
    let expected = book
        .exact_in(o.token_in, o.amount_in, false)
        .ok_or_else(|| fail(ctx))?
        .credit;
    if expected < o.min_out
        || bundles::official(ctx, e, &book, o.token_in, o.amount_in, &at).await? != expected
    {
        return Err(fail(ctx));
    }
    let order = sim::Order {
        payer,
        target,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: sim::Wire::Bundles(t),
    };
    let deadline = U256::from(
        head.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| fail(ctx))?,
    );
    let proof = t.proof(&order, o.min_out, deadline);
    sim::measure(ctx, head, order, true).await?;
    let approvals = if target == t.router {
        let have = dagrow::allowance(ctx, o.token_in, payer, target, &at).await?;
        approve::fork_exact(ctx, o.token_in, target, Some(have), o.amount_in)?
    } else {
        approve::fork_missing(ctx, o.token_in, payer, target, o.amount_in).await?
    };
    if generation != ctx.settings.generation() {
        return Err(fail(ctx));
    }
    let mut plan = Plan::new(
        ctx,
        (approvals, target, payer),
        o.token_out,
        vec![],
        o.min_out,
        deadline,
    )?;
    plan.set_direct_market(DirectCall::Bundles(std::sync::Arc::new(proof)));
    Ok(plan)
}
pub(in crate::venues::bsc_dex) fn probe(
    ctx: &Ctx,
    e: &Entry,
    o: &Order,
    wallet: Address,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, Bytes)> {
    let PoolState::Bundles(book) = &e.pool.state else {
        return Err(fail(ctx));
    };
    let router = ctx.dex_addr("dex.quoter", e.family.id)?;
    let target = if exact_wallet_tokens::eligible(ctx, e.family.id, o.token_in, o.token_out) {
        router
    } else {
        ctx.dex_addr("dex.router", e.family.id)?
    };
    let t = terms(ctx, e, book, target, o.token_in, o.amount_in)?;
    let order = sim::Order {
        payer: wallet,
        target,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: sim::Wire::Bundles(t),
    };
    let deadline = now
        .checked_add(ctx.int("dex.deadline_s")?)
        .ok_or_else(|| fail(ctx))?;
    let proof = t.proof(&order, o.min_out, deadline.into());
    Ok((
        approve::fork_exact(ctx, o.token_in, target, None, o.amount_in)?,
        target,
        proof.data(),
    ))
}
