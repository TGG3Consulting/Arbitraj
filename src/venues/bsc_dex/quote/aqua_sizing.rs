//! SwapVM допускает произвольные программы. Обратная котировка предлагает вход, оплата подтверждает его для данного способа.
use super::{
    Quoting,
    routing::Input,
    v2_execution::Execution,
    v2_wallet_sim::{self, Fill, Order},
    white_state::Head,
};
use crate::{
    core::types::{PoolState, Side},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        quote_stage,
    },
    venues::bsc_dex::{
        node::rpc,
        num,
        pools::{
            aqua::{self, IAqua},
            aqua_directory,
            registry::Entry,
        },
    },
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::json;

pub(super) async fn paid(
    q: &Quoting,
    i: &Input,
    e: &Entry,
    payer: Address,
    exec: &Execution,
    head: Head,
) -> GatewayResult<Fill> {
    let ctx = &q.ctx;
    let fail = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Aqua: на заданный объём не подтверждена чистая выплата из средств плательщика",
        )
    };
    let buy = i.side == Side::Buy;
    let (input, output) = if buy {
        (i.usdt, i.coin)
    } else {
        (i.coin, i.usdt)
    };
    let qty = if buy {
        super::direct::ceil_units(i.qty, i.coin_dec)
    } else {
        num::to_units(i.qty, i.coin_dec)
    }
    .filter(|v| !v.is_zero())
    .ok_or_else(fail)?;
    let measure = |amount| {
        v2_wallet_sim::measure(
            ctx,
            head,
            Order {
                payer,
                target: exec.target,
                input,
                output,
                amount,
                minimum: if buy { qty } else { U256::ONE },
                wire: exec.wire.clone(),
            },
            true,
        )
    };
    if !buy {
        return measure(qty).await;
    }
    let cap = quote_stage::buy_budget(ctx.place.id, input)
        .map(|d| num::to_units(d, i.usdt_dec).ok_or_else(fail))
        .transpose()?;
    let PoolState::OraclePmm(depth) = &e.pool.state else {
        return Err(fail());
    };
    let levels = if (input == e.pool.token0) == depth.base_is_token0 {
        &depth.ladder_base_in
    } else {
        &depth.ladder_quote_in
    };
    let mut sizes: Vec<U256> = levels
        .iter()
        .filter(|p| p.amount_out >= qty)
        .map(|p| p.amount_in)
        .collect();
    let offer = aqua_directory::identity(
        ctx,
        e.family,
        &e.pool.reference,
        (e.pool.token0, e.pool.token1),
    )?;
    let engine = ctx.dex_addr("dex.quoter", e.family.id)?;
    let mut taker = aqua::traits(
        cap.unwrap_or(U256::MAX),
        payer,
        aqua::MAX_DEADLINE,
        offer.first,
    )
    .ok_or_else(fail)?
    .to_vec();
    // TakerTraits v1.0.2: последний бит младшего байта flags означает exact input.
    taker[21] &= !1;
    let call = IAqua::quoteCall {
        order: offer.program,
        tokenIn: input,
        tokenOut: output,
        amount: qty,
        taker: taker.into(),
    };
    let raw = rpc::call::<Bytes>(
        ctx,
        "eth_call",
        json!([{"from":payer,"to":engine,"data":Bytes::from(call.abi_encode())},head.selector()]),
    )
    .await;
    if let Ok(raw) = raw
        && let Ok(hint) = IAqua::quoteCall::abi_decode_returns_validate(&raw)
        && hint.h == offer.hash
        && hint.net == qty
        && !hint.paid.is_zero()
    {
        sizes.push(hint.paid);
    }
    sizes.sort_unstable();
    sizes.dedup();
    for amount in sizes
        .into_iter()
        .filter(|amount| cap.is_none_or(|budget| *amount <= budget))
    {
        if let Ok(fill) = measure(amount).await {
            return Ok(fill);
        }
    }
    Err(fail())
}
