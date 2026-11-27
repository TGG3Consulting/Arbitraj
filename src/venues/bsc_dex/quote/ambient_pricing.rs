use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    v2_wallet_sim::{self, Fill, Order, V5Step, Wire},
    white_state::Head,
};
use crate::{
    core::types::{RouteMode, Side},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        quote_stage,
    },
    venues::bsc_dex::{
        exec::{ambient_plan, executor},
        num,
        pools::{ambient, dagrow, registry::Entry},
    },
};
use alloy::sol_types::SolValue;
use alloy_primitives::Address;
pub(super) async fn options(
    q: &Quoting,
    i: &Input,
    e: &Entry,
    payer: Address,
    native: bool,
    graph: bool,
    head: Option<Head>,
) -> Vec<OptionQuote> {
    if native {
        return vec![];
    }
    let Some(s) = ambient::shape(e) else {
        return vec![];
    };
    let Ok(target) = q.ctx.dex_addr("dex.router", e.family.id) else {
        return vec![];
    };
    let Ok(t) = ambient_plan::terms(&q.ctx, e, target, None) else {
        return vec![];
    };
    let input = if i.side == Side::Buy { i.usdt } else { i.coin };
    if let Some(h) = head
        && ambient_plan::check_module(&q.ctx, e, t, &h.selector())
            .await
            .is_err()
    {
        return vec![];
    }
    let one = |desired| {
        v2_execution::option(
            q,
            i,
            e.clone(),
            payer,
            Execution {
                mode: RouteMode::Direct,
                target,
                wire: Wire::Ambient(crate::venues::bsc_dex::exec::ambient_wallet::Terms {
                    desired,
                    ..t
                }),
            },
            head,
        )
    };
    let (source, by_target, via) = tokio::join!(
        one(None),
        async {
            if i.side != Side::Buy || head.is_none() {
                return None;
            }
            one(Some(super::direct::ceil_units(i.qty, i.coin_dec)?))
                .await
                .ok()
        },
        async {
            if !graph {
                return None;
            }
            let target_exec = executor::address(&q.ctx).ok()??;
            if let Some(h) = head {
                ambient_plan::graph(&q.ctx, e, input, &h.selector())
                    .await
                    .ok()?
            }
            let v = V5Step {
                kind: 17,
                pool: target,
                data: s.market.id().abi_encode().into(),
                native: None,
            };
            v2_execution::option(
                q,
                i,
                e.clone(),
                payer,
                Execution {
                    mode: RouteMode::Graph,
                    target: target_exec,
                    wire: Wire::V5(v),
                },
                head,
            )
            .await
            .ok()
        }
    );
    source
        .ok()
        .into_iter()
        .chain(by_target)
        .chain(via)
        .collect()
}
pub(super) async fn paid(
    q: &Quoting,
    i: &Input,
    e: &Entry,
    payer: Address,
    exec: &Execution,
    h: Head,
) -> GatewayResult<Fill> {
    let by_target = matches!(exec.wire,Wire::Ambient(t) if t.desired.is_some());
    let fail = || {
        q.ctx.err(
            ErrorCode::NoRoute,
            "Ambient: фактический расход/выход не проходит порог",
        )
    };
    if i.side == Side::Sell {
        return super::yield_basis_sizing::paid(q, i, e, payer, exec, h).await;
    }
    let qty = super::direct::ceil_units(i.qty, i.coin_dec).ok_or_else(fail)?;
    let cap = quote_stage::buy_budget(q.ctx.place.id, i.usdt)
        .map(|v| num::to_units(v, i.usdt_dec).ok_or_else(fail))
        .transpose()?;
    let bal = dagrow::balance(&q.ctx, i.usdt, payer, &h.selector()).await?;
    let max = cap.unwrap_or(bal).min(bal);
    let preview = ambient::official(&q.ctx, e, i.usdt, qty, false, &h.selector())
        .await
        .ok()
        .map(|p| p.amount_in)
        .filter(|n| *n <= max);
    let budgets = [
        preview,
        (by_target && cap.is_some() && !max.is_zero() && Some(max) != preview).then_some(max),
    ];
    for amount in budgets.into_iter().flatten() {
        let order = Order {
            payer,
            target: exec.target,
            input: i.usdt,
            output: i.coin,
            amount,
            minimum: qty,
            wire: exec.wire.clone(),
        };
        if let Ok(fill) = v2_wallet_sim::measure(&q.ctx, h, order, true).await {
            return Ok(fill);
        }
    }
    if by_target {
        return Err(fail());
    }
    super::yield_basis_sizing::paid(q, i, e, payer, exec, h).await
}
