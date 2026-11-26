//! Один физический EtherVista WETH/токен. Стороны по нативному контракту и тикеру
//! кошелька определяют вид платежа; суммы остаются в единицах исходных двух активов.
use super::super::{
    ctx::Ctx,
    exec::{
        native,
        vista_wire::{Call, Form},
    },
    pools::{
        ethervista::{self, VistaMarket},
        family_variant::Variant,
        registry::Entry,
    },
    wallet::signer,
};
use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    v2_wallet_sim::Wire,
    white_state::Head,
};
use crate::{
    core::types::{PoolState, RouteMode, Side},
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy_primitives::Address;

pub(in crate::venues::bsc_dex) fn is_vista(e: &Entry) -> bool {
    e.family.variant == Variant::EtherVista
}
pub(in crate::venues::bsc_dex) fn form(native: bool, input: Address, wrapped: Address) -> Form {
    if !native {
        Form::ModuleToken
    } else if input == wrapped {
        Form::OfficialEthIn
    } else {
        Form::ModuleEthOut
    }
}
pub(in crate::venues::bsc_dex) async fn market(
    ctx: &Ctx,
    e: &Entry,
    h: Head,
    form: Form,
) -> GatewayResult<VistaMarket> {
    ethervista::at(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.pool.token0, e.pool.token1),
        &h.selector(),
        form != Form::OfficialEthIn,
    )
    .await
}
pub(in crate::venues::bsc_dex) fn wire(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    m: &VistaMarket,
    form: Form,
) -> GatewayResult<(Address, Call)> {
    let buy = input == m.wrapped;
    let invalid = || {
        ctx.err(
            ErrorCode::NoRoute,
            "EtherVista: комиссия, единица входа или платёжный модуль не подтверждены",
        )
    };
    if input != e.pool.token0 && input != e.pool.token1 {
        return Err(invalid());
    }
    let fee = if buy { m.buy_fee } else { m.sell_fee };
    let target = match form {
        Form::OfficialEthIn if buy => m.native,
        Form::ModuleEthOut if !buy => m.adapter.ok_or_else(invalid)?,
        Form::ModuleToken if buy || m.fee_reserve >= fee => m.adapter.ok_or_else(invalid)?,
        _ => return Err(invalid()),
    };
    if target.is_zero() {
        return Err(invalid());
    }
    Ok((
        target,
        Call {
            form,
            wrapped: m.wrapped,
            fee,
        },
    ))
}
pub(super) async fn candidates(q: &Quoting, i: &Input, h: Option<Head>) -> Vec<OptionQuote> {
    let Ok(payer) = signer::address(&q.ctx) else {
        return Vec::new();
    };
    let is_native = native::is_native(&q.ctx, &i.asset);
    let input = if i.side == Side::Buy { i.usdt } else { i.coin };
    let mut out = Vec::new();
    for entry in i.entries.iter().filter(|e| is_vista(e)) {
        let mut e = entry.clone();
        let PoolState::EtherVista(cached) = &e.pool.state else {
            continue;
        };
        let shape = form(is_native, input, cached.wrapped);
        if !crate::gateway::quote_stage::settlement()
            && e.family.executor_kind.is_some()
            && q.ctx.text("dex.router_mode").ok().as_deref() == Some("graph")
            && (!is_native || super::super::exec::executor::native_capable(&q.ctx))
        {
            let mut g = e.clone();
            let target = super::super::exec::executor::address(&q.ctx).ok().flatten();
            let module = if let Some(head) = h {
                if let Ok(m) = market(&q.ctx, &e, head, Form::ModuleToken).await {
                    g.pool.state = m.state();
                    g.pool.block = head.number.to();
                    wire(&q.ctx, &e, input, &m, Form::ModuleToken)
                        .ok()
                        .map(|(module, _)| module)
                } else {
                    None
                }
            } else {
                q.ctx.dex_addr("dex.router", e.family.id).ok()
            };
            if let (Some(target), Some(module)) = (target, module) {
                let v5 = super::v2_wallet_sim::V5Step {
                    native: is_native.then_some((cached.wrapped, input == cached.wrapped)),
                    kind: 29,
                    pool: module,
                    data: alloy_primitives::Bytes::new(),
                };
                if let Ok(option) = v2_execution::option(
                    q,
                    i,
                    g,
                    payer,
                    Execution {
                        mode: RouteMode::Graph,
                        target,
                        wire: Wire::V5(v5),
                    },
                    h,
                )
                .await
                {
                    out.push(option);
                }
            }
        }
        let (target, call) = if let Some(head) = h {
            let Ok(m) = market(&q.ctx, &e, head, shape).await else {
                continue;
            };
            let Ok(bound) = wire(&q.ctx, &e, input, &m, shape) else {
                continue;
            };
            e.pool.state = m.state();
            e.pool.block = head.number.to();
            bound
        } else {
            let key = if shape == Form::OfficialEthIn {
                "dex.quoter"
            } else {
                "dex.router"
            };
            let Ok(target) = q.ctx.dex_addr(key, e.family.id) else {
                continue;
            };
            if target.is_zero() {
                continue;
            }
            (
                target,
                Call {
                    form: shape,
                    wrapped: cached.wrapped,
                    fee: if input == cached.wrapped {
                        cached.buy_fee
                    } else {
                        cached.sell_fee
                    },
                },
            )
        };
        let exec = Execution {
            mode: RouteMode::Direct,
            target,
            wire: Wire::Vista(call),
        };
        if let Ok(option) = v2_execution::option(q, i, e, payer, exec, h).await {
            out.push(option);
        }
    }
    out
}

/// Проверяет аванс ETH, разрешения, газ и публикацию на балансе плательщика перед допуском.
pub(super) async fn funded(q:&Quoting, head:Head, payer:Address, value:alloy_primitives::U256, fill:&super::v2_wallet_sim::Fill, floor:alloy_primitives::U256)->GatewayResult<()> {
    use super::super::{exec::gas_fund, node::rpc, send::{self,tx,tx_price}};
    use alloy_primitives::{U256,U64};
    let ctx=&q.ctx;
    let (fees,base)=q.gas.fees(ctx,&q.heads).await?;
    let price=tx_price::floored(fees,floor,base).ok_or_else(||ctx.err(ErrorCode::BadResponse,"Недоступна предельная цена газа EtherVista"))?;
    let margin=ctx.dec("send.gas_limit_margin_pct")?;
    let tip_cost=if let Some(t)=send::planned_tip(ctx) {
        let units=send::tip_gas::planned_units(ctx).await?.ok_or_else(||ctx.err(ErrorCode::StaleData,"Газ перевода чаевых EtherVista неизвестен"))?;
        Some((t,tx::with_margin(ctx,U64::from(units),margin)?))
    } else {None};
    let swap=tx::with_margin(ctx,U64::from(fill.swap_gas),margin)?;
    let approvals=tx::with_margin(ctx,U64::from(fill.approval.0),margin)?;
    let total=swap.saturating_add(approvals).saturating_add(fill.approval.1.saturating_sub(1) as u64);
    let gas=total.saturating_add(tip_cost.map_or(0,|(_,g)|g));
    let extra=q.gas.extra(ctx,gas,1+fill.approval.1+usize::from(tip_cost.is_some()))?;
    let need=gas_fund::need_wei(value,U256::from(price.max_fee),total,tip_cost).saturating_add(extra);
    let need=gas_fund::add_native_reserve(ctx,value,need)?;
    let available:U256=rpc::call(ctx,"eth_getBalance",serde_json::json!([payer,head.selector()])).await?;
    if available < need {return Err(ctx.err(ErrorCode::InsufficientBalance,"EtherVista: ETH кошелька не покрывает аванс, резерв, разрешения и газ"));}
    Ok(())
}
