//! Платёжный план EtherVista. Для нативного выхода контракт возвращает владельцу аванс,
//! проверяет минимальную чистую выплату и восстанавливает исходный остаток WETH роутера.
use super::super::{
    ctx::Ctx,
    quote::{vista_pricing, white_state},
    wallet::approve,
};
use super::{
    chain::Step,
    plan::{DirectCall, Plan},
    vista_wire::{Call, Form},
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy_primitives::{Address, U256};
tokio::task_local! {pub(super) static NATIVE:(bool,bool);}
pub(super) fn native() -> bool {
    NATIVE.try_with(|v| v.0).unwrap_or(false)
}

pub(super) async fn plan(
    ctx: &Ctx,
    steps: &[Step],
    wallet: Address,
    now: u64,
    all: bool,
) -> GatewayResult<Plan> {
    let [s] = steps else {
        return Err(ctx.err(ErrorCode::NoRoute, "Прямой EtherVista требует один пул"));
    };
    if !s.to_wallet || !s.from_wallet {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Обе стороны EtherVista должны принадлежать кошельку",
        ));
    }
    let generation = ctx.settings.generation();
    let o = &s.order;
    let wrapped = ctx.addr("dex.native_wrapped")?;
    let h = white_state::head(ctx, "latest").await?;
    let (target, call) =
        if s.entry.family.variant == super::super::pools::family_variant::Variant::BunnySwap {
            super::super::pools::bunnyswap::native_state(ctx, &s.entry, &h.selector()).await?;
            vista_pricing::rabbit_wire(ctx, &s.entry, o.token_in)?
        } else {
            let mut form = vista_pricing::form(native(), o.token_in, wrapped);
            if NATIVE.try_with(|v| v.1).unwrap_or(false) {
                if form != Form::ModuleEthOut {
                    return Err(ctx.err(
                        ErrorCode::NoRoute,
                        "Выбранный нативный выход EtherVista не соответствует токенам",
                    ));
                }
                form = Form::OfficialEthOut;
            }
            let m = vista_pricing::market(ctx, &s.entry, h, form).await?;
            vista_pricing::wire(ctx, &s.entry, o.token_in, &m, form)?
        };
    let form = call.form;
    if form == Form::OfficialEthOut && o.min_out.checked_add(call.fee).is_none() {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            "Валовой минимум EtherVista не помещается в wei",
        ));
    }
    let approvals = if matches!(form, Form::OfficialEthIn | Form::RabbitEthIn) {
        Vec::new()
    } else if all {
        approve::fork_all(ctx, o.token_in, target, o.amount_in)?
    } else {
        approve::fork_missing(ctx, o.token_in, wallet, target, o.amount_in).await?
    };
    if generation != ctx.settings.generation() {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Настройки EtherVista изменились до подготовки",
        ));
    }
    let deadline = U256::from(now.saturating_add(ctx.int("dex.deadline_s")?));
    let mut p = Plan::new(
        ctx,
        (approvals, target, wallet),
        o.token_out,
        vec![],
        o.min_out,
        deadline,
    )?;
    p.direct_market = Some(DirectCall::Vista {
        call,
        input: o.token_in,
        amount: o.amount_in,
    });
    Ok(p)
}
pub(in crate::venues::bsc_dex) async fn probe(
    ctx: &Ctx,
    e: &super::super::pools::registry::Entry,
    o: super::plan::Order,
    wallet: Address,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, alloy_primitives::Bytes)> {
    let p = plan(ctx, &super::plan::single(e, o), wallet, now, true).await?;
    let data = p.data(U256::ZERO);
    Ok((p.approvals, p.router, data))
}
pub(super) fn proof(plan: &Plan) -> Option<super::vista_fact::Proof> {
    let Some(DirectCall::Vista {
        call:
            call @ Call {
                form:
                    Form::OfficialEthIn
                    | Form::ModuleEthOut
                    | Form::OfficialEthOut
                    | Form::RabbitEthIn
                    | Form::RabbitEthOut,
                ..
            },
        input,
        amount,
    }) = plan.direct_market
    else {
        return None;
    };
    Some(super::vista_fact::Proof {
        call,
        target: plan.router,
        wallet: plan.wallet,
        input,
        output: plan.token_out,
        amount,
        minimum: plan.min_total(),
        data_hash: alloy_primitives::keccak256(plan.data(U256::ZERO)),
    })
}
