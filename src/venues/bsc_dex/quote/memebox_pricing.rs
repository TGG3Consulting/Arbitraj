//! Сравнивает прямой вызов MemeboxAdapter и шаг V5 по чистой выплате и расходам текущего кошелька.
use super::super::{
    exec::executor,
    pools::{family_variant::Variant, memebox, registry::Entry},
    wallet::signer,
};
use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    v2_wallet_sim::{V5Step, Wire},
    white_state::Head,
};
use crate::{core::types::RouteMode, gateway::quote_stage};
use alloy_primitives::Bytes;

pub(super) fn enabled(e: &Entry) -> bool {
    e.family.variant == Variant::Memebox && e.family.tradable && e.family.executor_kind.is_some()
}

pub(super) async fn candidates(q: &Quoting, i: &Input, head: Option<Head>) -> Vec<OptionQuote> {
    let ctx = &q.ctx;
    let Ok(payer) = signer::address(ctx) else {
        return Vec::new();
    };
    let graph =
        !quote_stage::settlement() && ctx.text("dex.router_mode").ok().as_deref() == Some("graph");
    let mut quotes = Vec::new();
    for e in i.entries.iter().filter(|e| enabled(e)) {
        let Ok(module) = ctx.dex_addr("dex.router", e.family.id) else {
            continue;
        };
        let mut e = e.clone();
        if let Some(h) = head
            && memebox::refresh(ctx, &mut e, h).await.is_err()
        {
            continue;
        }
        let direct = Execution {
            mode: RouteMode::Direct,
            target: module,
            wire: Wire::DirectModule {
                pool: e.pool.reference.address(),
            },
        };
        if let Ok(option) = v2_execution::option(q, i, e.clone(), payer, direct, head).await {
            quotes.push(option);
        }
        if graph && let Ok(Some(target)) = executor::address(ctx) {
            let step = V5Step {
                native: None,
                kind: 29,
                pool: module,
                data: Bytes::new(),
            };
            let via = Execution {
                mode: RouteMode::Graph,
                target,
                wire: Wire::V5(step),
            };
            if let Ok(option) = v2_execution::option(q, i, e.clone(), payer, via, head).await {
                quotes.push(option);
            }
        }
    }
    quotes
}
