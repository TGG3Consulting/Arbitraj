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
pub(in crate::venues::bsc_dex) fn native_router(e: &Entry) -> bool {
    matches!(e.family.variant, Variant::EtherVista | Variant::BunnySwap)
}
pub(in crate::venues::bsc_dex) fn rabbit_wire(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
) -> GatewayResult<(Address, Call)> {
    let (friend, wrapped) = super::super::pools::bunnyswap::tokens(ctx, e.family)?;
    let form = if input == wrapped {
        Form::RabbitEthIn
    } else if input == friend {
        Form::RabbitEthOut
    } else {
        return Err(ctx.err(ErrorCode::NoRoute, "RabbitRouter: неизвестная сторона пула"));
    };
    Ok((
        ctx.dex_addr("dex.quoter", e.family.id)?,
        Call {
            form,
            wrapped,
            fee: alloy_primitives::U256::ZERO,
        },
    ))
}
pub(in crate::venues::bsc_dex) use super::vista_cash::{cash, cash_market, terminal_cash};
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
        matches!(form, Form::ModuleEthOut | Form::ModuleToken),
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
        Form::OfficialEthOut if !buy && m.router_weth.is_zero() => m.native,
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
    let native_cash = cash(&q.ctx, &i.usdt_ticker, i.usdt);
    let is_native = native_cash || native::is_native(&q.ctx, &i.asset);
    let input = if i.side == Side::Buy { i.usdt } else { i.coin };
    let mut out = Vec::new();
    for entry in i.entries.iter().filter(|e| native_router(e)) {
        if entry.family.variant == Variant::BunnySwap {
            if !is_native {
                continue;
            }
            let mut e = entry.clone();
            if let Some(head) = h {
                let Ok(state) =
                    super::super::pools::bunnyswap::native_state(&q.ctx, &e, &head.selector())
                        .await
                else {
                    continue;
                };
                e.pool.state = state;
                e.pool.block = head.number.to();
            }
            let Ok((target, call)) = rabbit_wire(&q.ctx, &e, input) else {
                continue;
            };
            let spec = Execution {
                mode: RouteMode::Direct,
                target,
                wire: Wire::Vista(call),
            };
            if let Ok(option) = v2_execution::option(q, i, e, payer, spec, h).await {
                out.push(option);
            }
            continue;
        }
        let e = entry.clone();
        let PoolState::EtherVista(cached) = &e.pool.state else {
            continue;
        };
        let shape = form(is_native, input, cached.wrapped);
        if !native_cash
            && !crate::gateway::quote_stage::settlement()
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
        let direct_forms = std::iter::once(shape)
            .chain((shape == Form::ModuleEthOut).then_some(Form::OfficialEthOut));
        for shape in direct_forms {
            let mut e = entry.clone();
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
                let key = if matches!(shape, Form::OfficialEthIn | Form::OfficialEthOut) {
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
    }
    out
}
