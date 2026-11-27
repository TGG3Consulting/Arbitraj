//! Исполнение MCV2_Bond с разрешением от фактического плательщика и неизменным количеством выпуска/сжигания.
use super::super::{
    ctx::Ctx,
    math::mint_club as math,
    node::rpc,
    pools::{mint_club, registry::Entry},
    quote::{v2_wallet_sim, white_state::Head},
    wallet::approve,
};
use super::{
    chain::Step,
    mint_club_wire::{Call, Protection},
    plan::{DirectCall, Order, Plan},
};
use crate::{
    core::types::{MintClubState, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::json;
pub(in crate::venues::bsc_dex) fn enabled(e: &Entry) -> bool {
    matches!(e.pool.state, PoolState::MintClub(_))
}
fn fail(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Mint Club: объём, разрешённый резерв или предел платежа не подтверждены",
    )
}

pub(in crate::venues::bsc_dex) async fn state(
    ctx: &Ctx,
    e: &Entry,
    h: Head,
) -> GatewayResult<MintClubState> {
    mint_club::market(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.coin, e.usdt),
        &h.selector(),
    )
    .await
}
pub(in crate::venues::bsc_dex) fn call(
    s: &MintClubState,
    input: Address,
    qty: U256,
    cap: U256,
    minimum: U256,
) -> Option<(Call, math::Fill)> {
    let mint = input == s.reserve;
    if !mint && input != s.token {
        return None;
    }
    let f = math::fill(s, input, !mint, qty)?;
    if qty.is_zero()
        || minimum.is_zero()
        || cap.is_zero()
        || f.swapped.amount_in > cap
        || f.swapped.amount_out < minimum
    {
        return None;
    }
    Some((
        Call {
            protection: None,
            token: s.token,
            reserve: s.reserve,
            mint,
            qty,
            limit: if mint { cap } else { minimum },
        },
        f,
    ))
}
pub(in crate::venues::bsc_dex) async fn check(
    ctx: &Ctx,
    bond: Address,
    c: Call,
    f: &math::Fill,
    h: Head,
) -> GatewayResult<()> {
    let cash = if c.mint {
        f.swapped.amount_in
    } else {
        f.swapped.amount_out
    };
    if mint_club::official(ctx, bond, c.token, c.qty, c.mint, &h.selector()).await?
        != (cash, f.royalty)
    {
        return Err(fail(ctx));
    }
    Ok(())
}
pub(in crate::venues::bsc_dex) fn probe(
    ctx: &Ctx,
    e: &Entry,
    o: &Order,
    payer: Address,
    protection: Option<Protection>,
) -> GatewayResult<(Vec<approve::Approval>, Address, Bytes)> {
    let PoolState::MintClub(s) = &e.pool.state else {
        return Err(fail(ctx));
    };
    let f = math::fill(s, o.token_in, true, o.amount_in).ok_or_else(|| fail(ctx))?;
    let qty = if o.token_in == s.reserve {
        f.swapped.amount_out
    } else {
        f.swapped.amount_in
    };
    let (mut c, _) = call(s, o.token_in, qty, o.amount_in, o.min_out).ok_or_else(|| fail(ctx))?;
    let bond = ctx.dex_addr("dex.factory", e.family.id)?;
    c.protection = protection;
    let target = protection.map_or(bond, |p| p.module);
    Ok((
        approve::fork_all(ctx, o.token_in, target, o.amount_in)?,
        target,
        c.data(payer),
    ))
}
pub(super) async fn plan(
    ctx: &Ctx,
    step: &Step,
    qty: U256,
    payer: Address,
    h: Head,
    selected: Option<Address>,
) -> GatewayResult<Plan> {
    if !step.from_wallet || !step.to_wallet {
        return Err(fail(ctx));
    }
    let generation = ctx.settings.generation();
    let s = state(ctx, &step.entry, h).await?;
    let bond = ctx.dex_addr("dex.factory", step.entry.family.id)?;
    let o = &step.order;
    let (mut c, f) = call(&s, o.token_in, qty, o.amount_in, o.min_out).ok_or_else(|| fail(ctx))?;
    if c.pair() != (o.token_in, o.token_out) {
        return Err(fail(ctx));
    }
    let target = if let Some(selected) = selected {
        c.protection = Some(super::mint_club_binding::module(ctx, &step.entry, selected, h).await?);
        selected
    } else {
        super::mint_club_binding::nominal(ctx, &step.entry, s.reserve, h).await?;
        bond
    };
    check(ctx, bond, c, &f, h).await?;
    let measure = v2_wallet_sim::Order {
        payer,
        target,
        input: o.token_in,
        output: o.token_out,
        amount: f.swapped.amount_in,
        minimum: o.min_out,
        wire: v2_wallet_sim::Wire::MintClub(c),
    };
    let fill = v2_wallet_sim::measure(ctx, h, measure, true).await?;
    if (c.mint || c.protection.is_none()) && fill.received != f.swapped.amount_out {
        return Err(fail(ctx));
    }
    let approvals = approve::fork_missing(ctx, o.token_in, payer, target, o.amount_in).await?;
    if ctx.settings.generation() != generation {
        return Err(fail(ctx));
    }
    // Родной ABI имеет границы цены, но не имеет срока включения.
    let mut p = Plan::new(
        ctx,
        (approvals, target, payer),
        o.token_out,
        vec![],
        o.min_out,
        c.protection.map_or(U256::ZERO, |p| p.deadline),
    )?;
    p.set_direct_market(DirectCall::MintClub(c));
    Ok(p)
}

pub(in crate::venues::bsc_dex) async fn funded(
    ctx: &Ctx,
    wallet: Address,
    input: Address,
    amount: U256,
    h: Head,
) -> GatewayResult<()> {
    use alloy::sol_types::SolCall;
    let data = Bytes::from(mint_club::IBond::balanceOfCall { account: wallet }.abi_encode());
    let out: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"to":input,"data":data},h.selector()]),
    )
    .await?;
    if out.len() != 32 || U256::from_be_slice(&out) < amount {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Mint Club: для пробы недостаточно собственного входного ERC20",
        ));
    }
    Ok(())
}
pub(in crate::venues::bsc_dex) fn after_probe(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    amount: U256,
    received: U256,
    observed: (
        Address,
        Option<Protection>,
        &[super::super::check::simulate::SimLog],
    ),
) -> GatewayResult<Option<Box<Entry>>> {
    let PoolState::MintClub(s) = &e.pool.state else {
        return Ok(None);
    };
    let (fill, next) = math::after(s, input, amount).ok_or_else(|| fail(ctx))?;
    if fill.amount_out != received {
        let (payer, protection, logs) = observed;
        let Some(p) = protection.filter(|p| p.bond == s.bond) else {
            return Err(fail(ctx));
        };
        if input != s.token || received.is_zero() || received > fill.amount_out {
            return Err(fail(ctx));
        }
        let call = Call {
            token: s.token,
            reserve: s.reserve,
            mint: false,
            qty: amount,
            limit: U256::ONE,
            protection: Some(p),
        };
        let proof = super::mint_club_fact::Proof {
            call,
            bond: s.bond,
            payer,
        };
        if proof.measured(logs) != Some((amount, received)) {
            return Err(fail(ctx));
        }
    }
    let mut e = Box::new(e.clone());
    e.pool.state = PoolState::MintClub(Box::new(next));
    Ok(Some(e))
}
pub(super) async fn path_plan(
    ctx: &Ctx,
    step: &Step,
    hop: &super::super::quote::path::Hop,
    wallet: Address,
    height: u64,
    selected: Option<Address>,
) -> GatewayResult<Plan> {
    let PoolState::MintClub(state) = &step.entry.pool.state else {
        return Err(fail(ctx));
    };
    let qty = if step.order.token_in == state.reserve {
        hop.swapped.amount_out
    } else {
        hop.swapped.amount_in
    };
    let h = super::super::quote::white_state::head(ctx, &format!("{height:#x}")).await?;
    Box::pin(plan(ctx, step, qty, wallet, h, selected)).await
}

pub(in crate::venues::bsc_dex) fn selected(leg: &crate::core::types::RouteLeg) -> Option<Address> {
    if let [h] = leg.hops.as_slice()
        && let Some(
            crate::core::types::RouterFeeMode::ProtectedWallet { target }
            | crate::core::types::RouterFeeMode::NativeWallet {target, ..}
            | crate::core::types::RouterFeeMode::UniswapV1 {target, ..}
            | crate::core::types::RouterFeeMode::FluxEth {target, ..}
            | crate::core::types::RouterFeeMode::CarbonWallet {
                module: Some(target),
                ..
            },
        ) = h.fee_mode
    {
        Some(target)
    } else {
        None
    }
}
