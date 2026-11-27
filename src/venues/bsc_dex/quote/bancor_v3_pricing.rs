//! Dates, units and fee exemptions are bound to the same network block as the two physical reserves.
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
        exec::{
            bancor_v3_plan::{self, Form},
            executor,
        },
        pools::{bancor_v3, registry::Entry},
    },
};
use alloy_primitives::{Address, Bytes, U256};

pub(super) async fn options(
    q: &Quoting,
    i: &Input,
    e: &Entry,
    payer: Address,
    native: bool,
    graph: bool,
    head: Option<Head>,
) -> Vec<OptionQuote> {
    let ctx = &q.ctx;
    let mut quotes = vec![];
    let mut base = e.clone();
    if let Some(h) = head {
        let Ok(state) = bancor_v3::load(
            ctx,
            e.family,
            &e.pool.reference,
            [e.pool.token0, e.pool.token1],
            &h.selector(),
        )
        .await
        else {
            return quotes;
        };
        base.pool.state = PoolState::BancorV3(Box::new(state));
    }
    let PoolState::BancorV3(state) = &base.pool.state else {
        return quotes;
    };
    let buy = i.side == Side::Buy;
    let input = if buy { i.usdt } else { i.coin };
    let desired = if buy {
        let Some(want) = super::direct::ceil_units(i.qty, i.coin_dec) else {
            return quotes;
        };
        Some(want)
    } else {
        None
    };
    for module in [false, true] {
        let form = Form {
            module,
            native,
            exempt: false,
            desired: None,
        };
        let Ok(mut terms) = bancor_v3_plan::terms(ctx, &base, state, input, form) else {
            continue;
        };
        let target = terms.contract;
        let eligible = if let Some(h) = head {
            let at = h.selector();
            if bancor_v3_plan::check(ctx, &base, state, &terms, payer, &at)
                .await
                .is_err()
            {
                continue;
            }
            bancor_v3::arb(ctx, state, if module { target } else { payer }, &at)
                .await
                .unwrap_or(false)
        } else {
            false
        };
        // The arb entry point has a different caller check and gas charge. An exemption preserves
        // the regular entry point, including small amounts whose regular fee rounds to zero.
        for exempt in [false, true].into_iter().filter(|arb| !*arb || eligible) {
            let mut entry = base.clone();
            terms.exemption = exempt;
            if exempt && let PoolState::BancorV3(s) = &mut entry.pool.state {
                for market in s.markets.iter_mut().flatten() {
                    market.fee_ppm = 0;
                }
            }
            let direct = |t| Execution {
                mode: RouteMode::Direct,
                target,
                wire: Wire::BancorV3(Box::new(t)),
            };
            let source =
                v2_execution::option(q, i, entry.clone(), payer, direct(terms.clone()), head);
            let exact = async {
                let quantity = desired?;
                let mut t = terms.clone();
                t.desired = Some(quantity);
                v2_execution::option(q, i, entry.clone(), payer, direct(t), head)
                    .await
                    .ok()
            };
            let (by_source, by_target) = tokio::join!(source, exact);
            quotes.extend(by_source.ok());
            quotes.extend(by_target);
            if module
                && graph
                && !native
                && exempt == eligible
                && let Ok(Some(executor)) = executor::address(ctx)
            {
                let Some(form) = state.v5_form(&e.pool.reference, input, terms.eth) else { continue };
                let step = V5Step {
                    native: None,
                    kind: 17,
                    pool: target,
                    data: Bytes::copy_from_slice(&U256::from(form).to_be_bytes::<32>()),
                };
                let via = Execution {
                    mode: RouteMode::Graph,
                    target: executor,
                    wire: Wire::V5(step),
                };
                if let Ok(q) = v2_execution::option(q, i, entry, payer, via, head).await {
                    quotes.push(q);
                }
            }
        }
    }
    quotes
}
