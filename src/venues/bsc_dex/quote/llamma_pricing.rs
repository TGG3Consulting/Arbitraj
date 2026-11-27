//! Compare the real step 9 debit and net with the protected original exchange_dy/exchange.
//! A payer balance caps the funds available. Only the explicit request can enlarge its spending budget.
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
        exec::{executor, llamma_plan},
        num,
        pools::{dagrow, llamma, llamma_quotes, registry::Entry},
    },
};
use alloy::sol_types::SolValue;
use alloy_primitives::{Address, U256};
pub(super) async fn paid(
    q: &Quoting,
    i: &Input,
    e: &Entry,
    payer: Address,
    exec: &Execution,
    h: Head,
) -> GatewayResult<Fill> {
    let fail = || {
        q.ctx.err(
            ErrorCode::NoRoute,
            "LLAMMA: фактический обмен не укладывается в заданные границы",
        )
    };
    let buy = i.side == Side::Buy;
    let (src, dst) = if buy {
        (i.usdt, i.coin)
    } else {
        (i.coin, i.usdt)
    };
    let qty = if buy {
        super::direct::ceil_units(i.qty, i.coin_dec)
    } else {
        num::to_units(i.qty, i.coin_dec)
    }
    .filter(|x| !x.is_zero())
    .ok_or_else(fail)?;
    let at = h.selector();
    let amount = if buy {
        let balance = dagrow::balance(&q.ctx, src, payer, &at).await?;
        let quoted = llamma_quotes::official(&q.ctx, e, src, qty, true, &at).await;
        if let Ok(point) = quoted {
            let hard = quote_stage::buy_budget(q.ctx.place.id, src)
                .map(|v| num::to_units(v, i.usdt_dec).ok_or_else(fail))
                .transpose()?
                .unwrap_or(point.amount_in);
            if point.amount_in > hard.min(balance) {
                return Err(fail());
            }
            point.amount_in
        } else if matches!(exec.wire,Wire::Llamma(t) if t.desired.is_some()) {
            return Err(fail());
        } else {
            return super::yield_basis_sizing::paid(q, i, e, payer, exec, h).await;
        }
    } else {
        qty
    };
    let candidate = Order {
        payer,
        target: exec.target,
        input: src,
        output: dst,
        amount,
        minimum: if buy { qty } else { U256::ONE },
        wire: exec.wire.clone(),
    };
    match v2_wallet_sim::measure(&q.ctx, h, candidate, true).await {
        Ok(fill) => Ok(fill),
        Err(_) if buy && !matches!(exec.wire,Wire::Llamma(t) if t.desired.is_some()) => {
            super::yield_basis_sizing::paid(q, i, e, payer, exec, h).await
        }
        Err(e) => Err(e),
    }
}
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
    let Some(s) = llamma::shape(e) else {
        return vec![];
    };
    let input = if i.side == Side::Buy { i.usdt } else { i.coin };
    let Some(side) = s.side(input) else {
        return vec![];
    };
    let wanted = (i.side == Side::Buy)
        .then(|| super::direct::ceil_units(i.qty, i.coin_dec))
        .flatten();
    if let Some(h) = head
        && llamma::check(&q.ctx, e, &h.selector()).await.is_err()
    {
        return vec![];
    }
    let graph_call = async {
        if !graph {
            return None;
        }
        let target = executor::address(&q.ctx).ok()??;
        let step = V5Step {
            native: None,
            kind: 9,
            pool: s.amm,
            data: (U256::from(side), U256::from(1 - side)).abi_encode().into(),
        };
        v2_execution::option(
            q,
            i,
            e.clone(),
            payer,
            Execution {
                mode: RouteMode::Graph,
                target,
                wire: Wire::LlammaV5(step, s.coins[0]),
            },
            head,
        )
        .await
        .ok()
    };
    let wallet = async {
        let Ok(target) = q.ctx.dex_addr("dex.router", e.family.id) else {
            return vec![];
        };
        let Ok(t) = llamma_plan::terms(&q.ctx, e, target, None) else {
            return vec![];
        };
        if let Some(h) = head
            && llamma_plan::check_module(&q.ctx, e, t, &h.selector())
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
                    wire: Wire::Llamma(super::super::exec::llamma_wallet::Terms { desired, ..t }),
                },
                head,
            )
        };
        let (source, by_target) = tokio::join!(one(None), async {
            if wanted.is_some() && head.is_some() {
                one(wanted).await.ok()
            } else {
                None
            }
        });
        source.ok().into_iter().chain(by_target).collect::<Vec<_>>()
    };
    let (graph, mut direct) = tokio::join!(graph_call, wallet);
    direct.extend(graph);
    direct
}
