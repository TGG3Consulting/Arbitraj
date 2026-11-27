//! Сравнение двух форм заявки SSP и шага V5 с тарифом фактического плательщика.
use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    v2_wallet_sim::{V5Step, Wire},
    white_state::Head,
};
use crate::{
    core::types::{PoolState, RouteMode, Side},
    venues::bsc_dex::{
        exec::{dagrow_wallet::Terms, executor},
        num,
        pools::{dagrow, exact_wallet_tokens, registry::Entry},
    },
};
use alloy_primitives::{Address, Bytes};
pub(super) async fn options(
    q: &Quoting,
    i: &Input,
    e: &Entry,
    payer: Address,
    graph: bool,
    head: Option<Head>,
) -> Vec<OptionQuote> {
    let mut e = e.clone();
    let mut out = Vec::new();
    let ctx = &q.ctx;
    if let Some(h) = head {
        let Ok(state) = dagrow::state(
            ctx,
            e.family,
            e.pool.reference.address(),
            Some((e.coin, e.usdt)),
            &h.selector(),
        )
        .await
        else {
            return out;
        };
        e.pool.state = PoolState::Dagrow(Box::new(state));
    }
    let PoolState::Dagrow(s) = &e.pool.state else {
        return out;
    };
    let buy = i.side == Side::Buy;
    let (input, output) = if buy {
        (i.usdt, i.coin)
    } else {
        (i.coin, i.usdt)
    };
    let native = e.family.executor_kind.is_none();
    if native
        && (s.payer != payer || !exact_wallet_tokens::eligible_input(ctx, e.family.id, output))
    {
        return out;
    }
    if native
        && let Some(h) = head
        && exact_wallet_tokens::check_input(ctx, e.family.id, output, &h.selector())
            .await
            .is_err()
    {
        return out;
    }
    let target = if native { s.pool } else { s.payer };
    let qty = if buy {
        super::direct::ceil_units(i.qty, i.coin_dec)
    } else {
        num::to_units(i.qty, i.coin_dec)
    };
    let Some(estimate) = qty.and_then(|n| s.fill(input, !buy, n)) else {
        return out;
    };
    for desired in [None, Some(estimate.amount_out)] {
        if let Some(h) = head {
            let n = desired.unwrap_or(estimate.amount_in);
            let Some(model) = s.fill(input, desired.is_none(), n) else {
                continue;
            };
            let official =
                dagrow::official(ctx, s, input, desired.is_some(), n, &h.selector()).await;
            let Ok(actual) = official else { continue };
            if actual
                != if desired.is_some() {
                    model.amount_in
                } else {
                    model.amount_out
                }
                || (desired.is_some() && model.amount_in != estimate.amount_in)
            {
                continue;
            }
        }
        let direct = Execution {
            mode: RouteMode::Direct,
            target,
            wire: Wire::Dagrow(Terms {
                pool: s.pool,
                stable: s.stable,
                target,
                desired,
            }),
        };
        if let Ok(option) = v2_execution::option(q, i, e.clone(), payer, direct, head).await {
            out.push(option)
        }
    }
    if graph
        && !native
        && let Ok(Some(executor)) = executor::address(ctx)
    {
        let wire = Wire::V5(V5Step {
            native: None,
            kind: 29,
            pool: target,
            data: Bytes::new(),
        });
        let option = Execution {
            mode: RouteMode::Graph,
            target: executor,
            wire,
        };
        if let Ok(option) = v2_execution::option(q, i, e.clone(), payer, option, head).await {
            out.push(option)
        }
    }
    out
}
