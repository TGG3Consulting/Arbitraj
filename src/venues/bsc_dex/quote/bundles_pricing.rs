//! Сравнение Router, защищённого прямого модуля и V5 на одинаковом входе.
use super::super::{
    exec::{bundles_plan, executor},
    num,
    pools::{bundles, exact_wallet_tokens, registry::Entry},
};
use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    v2_wallet_sim::{V5Step, Wire},
    white_state::Head,
};
use crate::core::types::{PoolState, RouteMode, Side};
use alloy_primitives::{Address, Bytes, U256};
pub(super) async fn options(
    q: &Quoting,
    i: &Input,
    entry: &Entry,
    payer: Address,
    graph: bool,
    head: Option<Head>,
) -> Vec<OptionQuote> {
    let mut e = entry.clone();
    let ctx = &q.ctx;
    let (input, output, desired) = if i.side == Side::Buy {
        (i.usdt, i.coin, super::direct::ceil_units(i.qty, i.coin_dec))
    } else {
        (i.coin, i.usdt, num::to_units(i.qty, i.coin_dec))
    };
    let mut out = Vec::new();
    if let Some(h) = head {
        let Ok(fresh) = bundles::book(
            ctx,
            e.family,
            e.pool.reference.address(),
            (input, output),
            &h.selector(),
        )
        .await
        else {
            return out;
        };
        e.pool.state = PoolState::Bundles(Box::new(fresh));
    }
    let PoolState::Bundles(book) = &e.pool.state else {
        return out;
    };
    let Some((s, fill)) = desired.and_then(|qty| book.swap(input, i.side == Side::Sell, qty))
    else {
        return out;
    };
    let Ok(router) = ctx.dex_addr("dex.quoter", e.family.id) else {
        return out;
    };
    if let Some(h) = head
        && bundles::official(ctx, &e, book, input, s.amount_in, &h.selector())
            .await
            .ok()
            != Some(fill.credit)
    {
        return out;
    }
    let native = exact_wallet_tokens::eligible(ctx, e.family.id, input, output);
    let module = e
        .family
        .executor_kind
        .and_then(|_| ctx.dex_addr("dex.router", e.family.id).ok())
        .filter(|x| !x.is_zero() && *x != router);
    for target in [native.then_some(router), module].into_iter().flatten() {
        let Ok(t) = bundles_plan::terms(ctx, &e, book, target, input, s.amount_in) else {
            continue;
        };
        if let Some(h) = head {
            let valid = if target == router {
                exact_wallet_tokens::check(ctx, e.family.id, input, output, &h.selector()).await
            } else {
                bundles::module(ctx, &e, target, &h.selector()).await
            };
            if valid.is_err() {
                continue;
            }
        }
        let opt = Execution {
            mode: RouteMode::Direct,
            target,
            wire: Wire::Bundles(t),
        };
        if let Ok(o) = v2_execution::option(q, i, e.clone(), payer, opt, head).await {
            out.push(o)
        }
        if graph
            && target != router
            && let Ok(Some(ex)) = executor::address(ctx)
        {
            let m = &book.market;
            let step = V5Step {
                native: None,
                kind: 17,
                pool: target,
                data: Bytes::copy_from_slice(m.pool.into_word().as_slice()),
            };
            let mut recipients = vec![m.pool];
            let burn = if input == book.bun {
                let Some(net) = fill.market.balances[0].checked_sub(m.balances[0]) else {
                    continue;
                };
                let Some(amount) = s.amount_in.checked_sub(net) else {
                    continue;
                };
                (amount != U256::ZERO).then(|| super::v2_wallet_sim::BurnFunding {
                    credited: vec![(m.pool, net)],
                    burner: m.pool,
                    amount,
                })
            } else {
                if input == book.weth && m.pool != book.fee_pair.pool {
                    recipients.push(book.fee_pair.pool)
                }
                None
            };
            let opt = Execution {
                mode: RouteMode::Graph,
                target: ex,
                wire: Wire::funded(step, recipients, burn),
            };
            if let Ok(o) = v2_execution::option(q, i, e.clone(), payer, opt, head).await {
                out.push(o)
            }
        }
    }
    out
}
