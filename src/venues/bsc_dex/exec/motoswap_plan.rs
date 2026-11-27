//! Выбор и проверка способа исполнения для одной зарегистрированной пары Motoswap.
//! Планировщик и симулятор сверяют выбранную цель с состоянием пула и разрешением кошелька.
#[path = "motoswap_path_wire.rs"]
mod path;
pub(in crate::venues::bsc_dex) use path::path_wire;

use super::{
    chain::Step,
    motoswap_wallet::{Terms, abi::IModule},
    plan::Plan,
};
use crate::{
    core::types::{PoolRef, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::call::canonical,
        pools::{direct_module, exact_wallet_tokens, motoswap, registry::Entry},
        quote::{v2_wallet_sim as sim, white_state},
        wallet::approve,
    },
};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::Value;
pub(crate) fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Motoswap: не подтверждены маршрут или форма оплаты",
    )
}
pub(crate) fn shape(e: &Entry) -> Option<Address> {
    let PoolRef::Contract(pool) = e.pool.reference else {
        return None;
    };
    (matches!(e.pool.state, PoolState::Motoswap(_))
        && e.family.variant.motoswap()
        && !pool.is_zero()
        && !e.pool.token0.is_zero()
        && e.pool.token0 < e.pool.token1)
        .then_some(pool)
}
pub(crate) fn graph_target(ctx: &Ctx, e: &Entry) -> Option<(B256, Address)> {
    let pool = shape(e)?;
    let addr = ctx.dex_addr("dex.router", e.family.id).ok()?;
    (!addr.is_zero() && direct_module::configured(ctx, e.family.id).is_ok())
        .then_some((pool.into_word(), addr))
}
pub(crate) async fn terms(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    target: Address,
    at: Option<&Value>,
) -> GatewayResult<Terms> {
    let id = e.family.id;
    let pool = shape(e).ok_or_else(|| bad(ctx))?;
    if ![e.pool.token0, e.pool.token1].contains(&input) || target.is_zero() {
        return Err(bad(ctx));
    }
    let (fee, core, fee_code) = motoswap::routing(ctx, id)?;
    if fee != ctx.dex_addr("dex.quoter", id)? {
        return Err(bad(ctx));
    }
    let module_hash = if target == fee {
        if !exact_wallet_tokens::eligible_input(ctx, id, input) {
            return Err(bad(ctx));
        }
        if let Some(at) = at {
            tokio::try_join!(
                motoswap::frozen(ctx, fee, at),
                exact_wallet_tokens::check_input(ctx, id, input, at)
            )?;
        } else if !matches!(&e.pool.state,PoolState::Motoswap(s) if s.fee_router_frozen) {
            return Err(bad(ctx));
        }
        None
    } else {
        if Some(target) != graph_target(ctx, e).map(|(_, a)| a) {
            return Err(bad(ctx));
        }
        let hash = if let Some(at) = at {
            let (hash, factory, router) = tokio::try_join!(
                direct_module::verified_hash(ctx, id, target, at),
                canonical(ctx, at, target, IModule::factoryCall {}),
                canonical(ctx, at, target, IModule::routerCall {})
            )?;
            if factory != ctx.dex_addr("dex.factory", id)? || router != fee {
                return Err(bad(ctx));
            }
            hash
        } else {
            *direct_module::configured(ctx, id)?
                .first()
                .ok_or_else(|| bad(ctx))?
        };
        Some(hash)
    };
    Ok(Terms {
        pool,
        target,
        fee,
        core,
        fee_code,
        module_hash,
    })
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
    let head = white_state::head(ctx, &format!("{height:#x}")).await?;
    let at = head.selector();
    let e = &s.entry;
    let target = selected.ok_or_else(|| bad(ctx))?;
    let order = &s.order;
    let (t, _) = tokio::try_join!(
        terms(ctx, e, order.token_in, target, Some(&at)),
        motoswap::state(
            ctx,
            e.family,
            e.pool.reference.address(),
            (e.pool.token0, e.pool.token1),
            &at
        )
    )?;
    let o = sim::Order {
        payer,
        target,
        input: order.token_in,
        output: order.token_out,
        amount: order.amount_in,
        minimum: order.min_out,
        wire: sim::Wire::Motoswap(t),
    };
    super::fluid_lite_plan::finish(ctx, generation, head, o).await
}
pub(crate) fn probe(
    ctx: &Ctx,
    e: &Entry,
    o: &super::plan::Order,
    payer: Address,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, Bytes)> {
    let (fee, core, fee_code) = motoswap::routing(ctx, e.family.id)?;
    let pool = shape(e).ok_or_else(|| bad(ctx))?;
    let (target, module_hash) = if let Some((_, target)) = graph_target(ctx, e) {
        (
            target,
            direct_module::configured(ctx, e.family.id)?
                .first()
                .copied(),
        )
    } else if matches!(&e.pool.state,PoolState::Motoswap(s) if s.fee_router_frozen)
        && exact_wallet_tokens::eligible_input(ctx, e.family.id, o.token_in)
    {
        (fee, None)
    } else {
        return Err(bad(ctx));
    };
    let t = Terms {
        pool,
        target,
        fee,
        core,
        fee_code,
        module_hash,
    };
    let proof = t.proof(
        &sim::Order {
            payer,
            target,
            input: o.token_in,
            output: o.token_out,
            amount: o.amount_in,
            minimum: o.min_out,
            wire: sim::Wire::Motoswap(t),
        },
        o.min_out,
        U256::from(
            now.checked_add(ctx.int("dex.deadline_s")?)
                .ok_or_else(|| bad(ctx))?,
        ),
    );
    Ok((
        approve::fork_exact(ctx, o.token_in, target, None, o.amount_in)?,
        target,
        proof.data(),
    ))
}
