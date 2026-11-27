//! Сравнение защищённых прямого и V5 обменов по фактической валюте, расходу и газу.
use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    v2_wallet_sim::{V5Step, Wire},
    white_state::Head,
};
use crate::{
    core::types::RouteMode,
    venues::bsc_dex::{
        exec::{defiplaza_plan, executor},
        pools::{defiplaza, registry::Entry},
    },
};
use alloy_primitives::{Address, Bytes};

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
    let Ok(target) = ctx.dex_addr("dex.router", e.family.id) else {
        return vec![];
    };
    let input = match i.side {
        crate::core::types::Side::Buy => i.usdt,
        crate::core::types::Side::Sell => i.coin,
    };
    let Ok(t) = defiplaza_plan::terms(ctx, e, input, target, native) else {
        return vec![];
    };
    let mut entry = e.clone();
    if let Some(h) = head {
        let at = h.selector();
        let Ok(state) = defiplaza::state(
            ctx,
            e.family,
            e.pool.reference.address(),
            (e.pool.token0, e.pool.token1),
            &at,
        )
        .await
        else {
            return vec![];
        };
        entry.pool.state = crate::core::types::PoolState::DefiPlaza(Box::new(state));
    }
    let direct = v2_execution::option(
        q,
        i,
        entry.clone(),
        payer,
        Execution {
            wire: Wire::DefiPlaza(t),
            mode: RouteMode::Direct,
            target,
        },
        head,
    );
    let graph_option = async {
        if !graph || native {
            return None;
        }
        let exec = executor::address(ctx).ok()??;
        let step = V5Step {
            native: None,
            kind: 29,
            pool: target,
            data: Bytes::new(),
        };
        v2_execution::option(
            q,
            i,
            entry,
            payer,
            Execution {
                wire: Wire::V5(step),
                mode: RouteMode::Graph,
                target: exec,
            },
            head,
        )
        .await
        .ok()
    };
    let (wallet, step) = tokio::join!(direct, graph_option);
    wallet.ok().into_iter().chain(step).collect()
}
