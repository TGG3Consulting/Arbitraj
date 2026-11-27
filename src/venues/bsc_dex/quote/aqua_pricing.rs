//! Сравнение исходного SwapVM, прямого защищённого модуля и зарегистрированной программы V5.
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
        exec::{aqua_plan, executor},
        pools::{exact_wallet_tokens, registry::Entry},
    },
};
use alloy_primitives::{Address, Bytes};
use std::sync::Arc;

pub(super) async fn options(
    q: &Quoting,
    i: &Input,
    e: &Entry,
    payer: Address,
    native: bool,
    graph: bool,
    h: Option<Head>,
) -> Vec<OptionQuote> {
    if native {
        return vec![];
    }
    let ctx = &q.ctx;
    let mut found = Vec::new();
    let engine = ctx.dex_addr("dex.quoter", e.family.id).ok();
    let wallet = ctx
        .dex_addr("dex.router", e.family.id)
        .ok()
        .filter(|t| Some(*t) != engine);
    let bare = engine
        .filter(|_| exact_wallet_tokens::eligible(ctx, e.family.id, e.pool.token0, e.pool.token1));
    for target in bare.into_iter().chain(wallet) {
        let t = if let Some(head) = h {
            aqua_plan::checked(ctx, e, target, &head.selector()).await
        } else {
            aqua_plan::terms(ctx, e, target)
        };
        let Ok(t) = t else { continue };
        let direct = Execution {
            mode: RouteMode::Direct,
            target,
            wire: Wire::Aqua(Arc::new(t)),
        };
        if let Ok(quote) = v2_execution::option(q, i, e.clone(), payer, direct, h).await {
            found.push(quote);
        }
    }
    if graph
        && let (Some(head), Some(exec), Some((key, module))) = (
            h,
            executor::address(ctx).ok().flatten(),
            aqua_plan::graph(ctx, e),
        )
        && aqua_plan::cached(ctx, e, head, exec).await.is_ok()
    {
        let step = V5Step {
            native: None,
            kind: 17,
            pool: module,
            data: Bytes::from(key.to_vec()),
        };
        let via = Execution {
            mode: RouteMode::Graph,
            target: exec,
            wire: Wire::V5(step),
        };
        if let Ok(quote) = v2_execution::option(q, i, e.clone(), payer, via, h).await {
            found.push(quote);
        }
    }
    found
}
