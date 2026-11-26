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
    mint_club_wire::Call,
    plan::{DirectCall, Order, Plan},
};
use crate::{
    core::types::{MintClubState, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy_primitives::{Address, B256, Bytes, U256, b256, keccak256};
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

// Обе проверенные реализации WETH переносят ровно номинал. Bond проверяет сумму до ERC20.transfer.
fn pays_nominal(hash: B256) -> bool {
    [
        b256!("8a3a1f6a9f9dce633117adee5b458245835a8645a8c8726a26382a4622508b1c"),
        b256!("d0a06b12ac47863b5c7be4185c2deaad1c61557033f56c7d4ea74429cbb25e23"),
    ]
    .contains(&hash)
}
pub(in crate::venues::bsc_dex) async fn state(
    ctx: &Ctx,
    e: &Entry,
    h: Head,
) -> GatewayResult<MintClubState> {
    let PoolState::MintClub(s) = &e.pool.state else {
        return Err(fail(ctx));
    };
    if ctx.addr("dex.native_wrapped")? != s.reserve {
        return Err(fail(ctx));
    }
    let at = h.selector();
    let (state, code) = tokio::try_join!(
        mint_club::market(
            ctx,
            e.family,
            e.pool.reference.address(),
            (e.coin, e.usdt),
            &at
        ),
        rpc::call::<Bytes>(ctx, "eth_getCode", json!([s.reserve, at]))
    )?;
    if state.reserve != s.reserve || !pays_nominal(keccak256(code)) {
        return Err(fail(ctx));
    }
    Ok(state)
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
    let (c, _) = call(s, o.token_in, qty, o.amount_in, o.min_out).ok_or_else(|| fail(ctx))?;
    let bond = ctx.dex_addr("dex.factory", e.family.id)?;
    Ok((
        approve::fork_all(ctx, o.token_in, bond, o.amount_in)?,
        bond,
        c.data(payer),
    ))
}
pub(super) async fn plan(
    ctx: &Ctx,
    step: &Step,
    qty: U256,
    payer: Address,
    h: Head,
) -> GatewayResult<Plan> {
    if !step.from_wallet || !step.to_wallet {
        return Err(fail(ctx));
    }
    let generation = ctx.settings.generation();
    let s = state(ctx, &step.entry, h).await?;
    let bond = ctx.dex_addr("dex.factory", step.entry.family.id)?;
    let o = &step.order;
    let (c, f) = call(&s, o.token_in, qty, o.amount_in, o.min_out).ok_or_else(|| fail(ctx))?;
    if c.pair() != (o.token_in, o.token_out) {
        return Err(fail(ctx));
    }
    check(ctx, bond, c, &f, h).await?;
    let measure = v2_wallet_sim::Order {
        payer,
        target: bond,
        input: o.token_in,
        output: o.token_out,
        amount: f.swapped.amount_in,
        minimum: o.min_out,
        wire: v2_wallet_sim::Wire::MintClub(c),
    };
    let fill = v2_wallet_sim::measure(ctx, h, measure, true).await?;
    if fill.received != f.swapped.amount_out {
        return Err(fail(ctx));
    }
    let approvals = approve::fork_missing(ctx, o.token_in, payer, bond, o.amount_in).await?;
    if ctx.settings.generation() != generation {
        return Err(fail(ctx));
    }
    // Родной ABI имеет границы цены, но не имеет срока включения.
    let mut p = Plan::new(
        ctx,
        (approvals, bond, payer),
        o.token_out,
        vec![],
        o.min_out,
        U256::ZERO,
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
) -> GatewayResult<Option<Box<Entry>>> {
    let PoolState::MintClub(s) = &e.pool.state else {
        return Ok(None);
    };
    let (fill, next) = math::after(s, input, amount).ok_or_else(|| fail(ctx))?;
    if fill.amount_out != received {
        return Err(fail(ctx));
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
    Box::pin(plan(ctx, step, qty, wallet, h)).await
}
