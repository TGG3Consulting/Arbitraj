//! Отдельная физическая пара Carbon и подписанная сторона ограничения. Повторное измерение перед отправкой использует тот же блок, распределение и плательщика.
use super::super::{
    ctx::Ctx,
    math::carbon_match,
    pools::{carbon, carbon_quote, registry::Entry},
    quote::{
        v2_wallet_sim,
        white_state::{self, Head},
    },
    wallet::approve,
};
use super::{
    carbon_wire::Call,
    chain::Step,
    plan::{DirectCall, Plan},
};
use crate::{
    core::types::{CarbonState, PoolRef, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy_primitives::{Address, U256};
use std::sync::Arc;
pub(in crate::venues::bsc_dex) fn enabled(e: &Entry) -> bool {
    matches!(e.pool.state, PoolState::Carbon(_))
}
fn fail(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Carbon: не подтверждены пара, чистая выплата или предел плательщика",
    )
}
pub(in crate::venues::bsc_dex) async fn state(
    ctx: &Ctx,
    e: &Entry,
    h: Head,
) -> GatewayResult<CarbonState> {
    let PoolRef::Singleton { manager, id } = &e.pool.reference else {
        return Err(fail(ctx));
    };
    carbon::market(
        ctx,
        e.family,
        *manager,
        *id,
        (e.coin, e.usdt),
        &h.selector(),
    )
    .await
}
#[allow(clippy::too_many_arguments)]
pub(in crate::venues::bsc_dex) fn call(
    ctx: &Ctx,
    s: &CarbonState,
    input: Address,
    by_target: bool,
    size: U256,
    cap: U256,
    minimum: U256,
    timestamp: u64,
) -> GatewayResult<(Call, carbon_match::Plan)> {
    let bad = || fail(ctx);
    let p = u128::try_from(size)
        .ok()
        .and_then(|n| carbon_match::plan(s, input, by_target, n))
        .ok_or_else(bad)?;
    if U256::from(p.spent) > cap || U256::from(p.received) < minimum {
        return Err(bad());
    }
    let limit = u128::try_from(if by_target { cap } else { minimum }).map_err(|_| bad())?;
    let deadline = U256::from(
        timestamp
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(bad)?,
    );
    let wire = Call::new(s, input, &p, limit, deadline).ok_or_else(bad)?;
    Ok((wire, p))
}
pub(in crate::venues::bsc_dex) async fn bound(
    ctx: &Ctx,
    e: &Entry,
    c: &mut Call,
    h: Head,
    protected: bool,
) -> GatewayResult<()> {
    use super::super::{node::rpc, pools::direct_module};
    use super::carbon_wire::IWallet;
    use alloy::sol_types::SolCall;
    use alloy_primitives::Bytes;
    use serde_json::json;
    let id = e.family.id;
    if ctx.dex_addr("dex.factory", id)? != c.controller {
        return Err(fail(ctx));
    }
    c.code = Some(super::carbon_binding::Code::profile(ctx, id)?);
    if !protected {
        let payer = super::super::wallet::signer::address(ctx)?;
        let wallet_code: Bytes =
            rpc::call(ctx, "eth_getCode", json!([payer, h.selector()])).await?;
        return if c.target == carbon::NATIVE && c.module.is_none() && wallet_code.is_empty() {
            Ok(())
        } else {
            Err(fail(ctx))
        };
    }
    let module = ctx.dex_addr("dex.router", id)?;
    let allowed = direct_module::configured(ctx, id)?;
    let (code, issuer): (Bytes, Bytes) = tokio::try_join!(
        rpc::call(ctx, "eth_getCode", json!([module, h.selector()])),
        rpc::call(
            ctx,
            "eth_call",
            json!([{"to":module,"data":Bytes::from(IWallet::controllerCall{}.abi_encode())},h.selector()])
        )
    )?;
    if module == c.controller
        || direct_module::matching_hash(&allowed, &code).is_none()
        || IWallet::controllerCall::abi_decode_returns_validate(&issuer).ok() != Some(c.controller)
    {
        return Err(fail(ctx));
    }
    c.module = Some(module);
    Ok(())
}
pub(super) async fn path_plan(
    ctx: &Ctx,
    s: &Step,
    hop: &super::super::quote::path::Hop,
    payer: Address,
    height: u64,
    by_target: bool,
    selected: (Option<Address>, Option<alloy_primitives::B256>),
) -> GatewayResult<Plan> {
    if !s.from_wallet || !s.to_wallet {
        return Err(fail(ctx));
    }
    let generation = ctx.settings.generation();
    let h = white_state::head(ctx, &format!("{height:#x}")).await?;
    let market = state(ctx, &s.entry, h).await?;
    let o = &s.order;
    let size = if by_target {
        hop.swapped.amount_out
    } else {
        o.amount_in
    };
    let (module, allocation) = selected;
    let plan = select(&market, o.token_in, by_target, size, allocation).ok_or_else(|| fail(ctx))?;
    let cap =
        u128::try_from(if by_target { o.amount_in } else { o.min_out }).map_err(|_| fail(ctx))?;
    let deadline = U256::from(
        h.timestamp
            .to::<u64>()
            .saturating_add(ctx.int("dex.deadline_s")?),
    );
    let mut c = Call::new(&market, o.token_in, &plan, cap, deadline).ok_or_else(|| fail(ctx))?;
    bound(ctx, &s.entry, &mut c, h, module.is_some()).await?;
    if c.output != o.token_out || c.module != module {
        return Err(fail(ctx));
    }
    carbon_quote::official(ctx, &market, o.token_in, &plan, &h.selector()).await?;
    let c = Arc::new(c);
    let order = v2_wallet_sim::Order {
        payer,
        target: c.recipient(),
        input: c.input,
        output: c.output,
        amount: U256::from(plan.spent),
        minimum: o.min_out,
        wire: v2_wallet_sim::Wire::Carbon(c.clone()),
    };
    let measured = v2_wallet_sim::measure(ctx, h, order, true).await?;
    if (c.module.is_none() || by_target) && measured.received != U256::from(plan.received) {
        return Err(fail(ctx));
    }
    let approvals = if c.source == carbon::NATIVE {
        vec![]
    } else {
        approve::fork_missing(ctx, c.input, payer, c.recipient(), c.cap()).await?
    };
    if ctx.settings.generation() != generation {
        return Err(fail(ctx));
    }
    let mut p = Plan::new(
        ctx,
        (approvals, c.recipient(), payer),
        c.output,
        vec![],
        o.min_out,
        c.deadline,
    )?;
    p.set_direct_market(DirectCall::Carbon(c));
    Ok(p)
}

pub(in crate::venues::bsc_dex) fn selected(
    leg: &crate::core::types::RouteLeg,
) -> (Option<Address>, Option<alloy_primitives::B256>) {
    if let [h] = leg.hops.as_slice()
        && let Some(crate::core::types::RouterFeeMode::CarbonWallet {
            module, allocation, ..
        }) = h.fee_mode
    {
        (module, allocation)
    } else {
        (None, None)
    }
}
fn select(
    s: &CarbonState,
    input: Address,
    buy: bool,
    size: U256,
    allocation: Option<alloy_primitives::B256>,
) -> Option<carbon_match::Plan> {
    carbon_match::alternatives(s, input, buy, u128::try_from(size).ok()?)
        .into_iter()
        .find(|p| {
            allocation.is_none_or(|wanted| {
                super::carbon_wire::fingerprint(p.actions.iter().map(|a| (a.id, a.amount)))
                    == wanted
            })
        })
}
pub(super) async fn adjust(
    ctx: &Ctx,
    path: &mut super::super::quote::path::Path,
    route: &[crate::core::types::RouteHop],
    buy: bool,
) -> GatewayResult<()> {
    let ([h], [r]) = (path.hops.as_mut_slice(), route) else {
        return Ok(());
    };
    let PoolState::Carbon(s) = &h.entry.pool.state else {
        return Ok(());
    };
    let Some(crate::core::types::RouterFeeMode::CarbonWallet {
        module, allocation, ..
    }) = r.fee_mode
    else {
        return Err(fail(ctx));
    };
    let full = if buy {
        h.swapped.amount_out
    } else {
        h.swapped.amount_in
    };
    let p = select(s, h.token_in, buy, full, allocation).ok_or_else(|| fail(ctx))?;
    h.swapped.amount_in = U256::from(p.spent);
    h.swapped.amount_out = U256::from(p.received);
    h.pool_in =
        super::super::num::to_decimal(h.swapped.amount_in, h.dec_in).ok_or_else(|| fail(ctx))?;
    h.pool_out =
        super::super::num::to_decimal(h.swapped.amount_out, h.dec_out).ok_or_else(|| fail(ctx))?;
    path.usdt = if buy {
        h.pool_in
    } else if let Some(module) = module {
        // На продаже через модуль фактический ERC20-кредит может отличаться от номинала контроллера.
        // От него считаются предел цены и проскальзывание при следующем пересчёте после approve.
        let head = white_state::head(ctx, "latest").await?;
        let fresh = state(ctx, &h.entry, head).await?;
        let plan = select(&fresh, h.token_in, buy, full, allocation).ok_or_else(|| fail(ctx))?;
        let deadline = U256::from(
            head.timestamp
                .to::<u64>()
                .saturating_add(ctx.int("dex.deadline_s")?),
        );
        let mut c = Call::new(&fresh, h.token_in, &plan, 1, deadline).ok_or_else(|| fail(ctx))?;
        bound(ctx, &h.entry, &mut c, head, true).await?;
        if c.module != Some(module) {
            return Err(fail(ctx));
        }
        let payer = super::super::wallet::signer::address(ctx)?;
        let actual = v2_wallet_sim::measure(
            ctx,
            head,
            v2_wallet_sim::Order {
                payer,
                target: module,
                input: c.input,
                output: c.output,
                amount: U256::from(plan.spent),
                minimum: U256::ONE,
                wire: v2_wallet_sim::Wire::Carbon(Arc::new(c)),
            },
            false,
        )
        .await?;
        super::super::num::to_decimal(actual.received, h.dec_out).ok_or_else(|| fail(ctx))?
    } else {
        h.pool_out
    };
    Ok(())
}
