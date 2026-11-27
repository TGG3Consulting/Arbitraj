//! Compare actual raw wallet receipts and approval gas for one registered MotoSwap pair.
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
        exec::{executor, motoswap_plan},
        pools::{motoswap, registry::Entry},
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
    let ctx = &q.ctx;
    let Some(pool) = motoswap_plan::shape(e) else {
        return vec![];
    };
    let Ok(fee) = ctx.dex_addr("dex.quoter", e.family.id) else {
        return vec![];
    };
    let input = if i.side == Side::Buy { i.usdt } else { i.coin };
    let at = head.map(|h| h.selector());
    let mut entry = e.clone();
    if let Some(at) = at.as_ref() {
        let Ok(state) =
            motoswap::state(ctx, e.family, pool, (e.pool.token0, e.pool.token1), at).await
        else {
            return vec![];
        };
        entry.pool.state = PoolState::Motoswap(Box::new(state));
    }
    let mut executions = Vec::new();
    if matches!(&entry.pool.state,PoolState::Motoswap(s) if s.fee_router_frozen)
        && let Ok(t) = motoswap_plan::terms(ctx, &entry, input, fee, at.as_ref()).await
    {
        executions.push(Execution {
            mode: RouteMode::Direct,
            target: fee,
            wire: Wire::Motoswap(t),
        });
    }
    if let Some((market, target)) = motoswap_plan::graph_target(ctx, &entry)
        && let Ok(t) = motoswap_plan::terms(ctx, &entry, input, target, at.as_ref()).await
    {
        executions.push(Execution {
            mode: RouteMode::Direct,
            target,
            wire: Wire::Motoswap(t),
        });
        if graph && let Ok(Some(exec)) = executor::address(ctx) {
            executions.push(Execution {
                mode: RouteMode::Graph,
                target: exec,
                wire: Wire::V5(V5Step {
                    native: None,
                    kind: 17,
                    pool: target,
                    data: Bytes::copy_from_slice(market.as_slice()),
                }),
            });
        }
    }
    futures_util::future::join_all(
        executions
            .into_iter()
            .map(|execution| v2_execution::option(q, i, entry.clone(), payer, execution, head)),
    )
    .await
    .into_iter()
    .filter_map(Result::ok)
    .collect()
}
