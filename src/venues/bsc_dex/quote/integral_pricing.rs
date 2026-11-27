//! Сравнение штатного Relayer, защищённой прямой заявки и шага V5 с расходами текущего плательщика.
use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    v2_wallet_sim::{V5Step, Wire},
    white_state::Head,
};
use crate::{
    core::types::{RouteMode, Side},
    venues::bsc_dex::{
        exec::{executor, integral_plan},
        pools::{exact_wallet_tokens, integral_size::Roles, registry::Entry},
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
    let Ok(r) = Roles::read(ctx, e.family.id) else {
        return vec![];
    };
    let buy = i.side == Side::Buy;
    let (input, output) = if buy {
        (i.usdt, i.coin)
    } else {
        (i.coin, i.usdt)
    };
    let desired = buy
        .then(|| super::direct::ceil_units(i.qty, i.coin_dec))
        .flatten();
    let mut options = Vec::new();
    let mut targets = Vec::new();
    if exact_wallet_tokens::eligible(ctx, e.family.id, input, output) {
        targets.push(r.relay)
    }
    if e.family.executor_kind.is_some()
        && let Ok(module) = ctx.dex_addr("dex.router", e.family.id)
    {
        targets.push(module)
    }
    for target in targets {
        if let Some(h) = head
            && integral_plan::checked(ctx, e, input, output, target, &h.selector())
                .await
                .is_err()
        {
            continue;
        }
        for want in std::iter::once(None).chain(desired.map(Some)) {
            let t = integral_plan::terms(e, r, target, want);
            let direct = Execution {
                mode: RouteMode::Direct,
                target,
                wire: Wire::Integral(t),
            };
            if let Ok(item) = v2_execution::option(q, i, e.clone(), payer, direct, head).await {
                options.push(item)
            }
        }
        if graph
            && target != r.relay
            && let Ok(Some(v5)) = executor::address(ctx)
        {
            let wire = Wire::V5(V5Step {
                native: None,
                kind: 29,
                pool: target,
                data: Bytes::new(),
            });
            let exec = Execution {
                mode: RouteMode::Graph,
                target: v5,
                wire,
            };
            if let Ok(item) = v2_execution::option(q, i, e.clone(), payer, exec, head).await {
                options.push(item)
            }
        }
    }
    options
}
