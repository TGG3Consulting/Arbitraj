//! Две альтернативы одного кошелька на одинаковом объёме. Ликвидность между альтернативами не суммируется.
use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    v2_wallet_sim::Wire,
    white_state::Head,
};
use crate::{
    core::types::{RouteMode, Side},
    venues::bsc_dex::{
        exec::prop_plan,
        pools::{exact_wallet_tokens, prop_venue, registry::Entry},
    },
};
use alloy_primitives::Address;
pub(super) async fn options(
    q: &Quoting,
    i: &Input,
    e: &Entry,
    payer: Address,
    head: Option<Head>,
) -> Vec<OptionQuote> {
    let ctx = &q.ctx;
    let Ok(r) = prop_venue::Roles::read(ctx, e.family.id) else {
        return vec![];
    };
    let (input, output) = if i.side == Side::Buy {
        (i.usdt, i.coin)
    } else {
        (i.coin, i.usdt)
    };
    let raw = exact_wallet_tokens::eligible_input(ctx, e.family.id, input).then_some(r.router());
    let strict = ctx
        .dex_addr("dex.router", e.family.id)
        .ok()
        .filter(|&m| m != r.router());
    let mut out = Vec::new();
    for (target, allow) in [
        raw.map(|t| (t, true)),
        strict.map(|t| (t, false)),
        strict.map(|t| (t, true)),
    ]
    .into_iter()
    .flatten()
    {
        let terms = match head {
            Some(h) => {
                prop_plan::checked(ctx, e, input, output, target, allow, &h.selector()).await
            }
            None => r.terms(ctx, target, allow),
        };
        let Ok(t) = terms else {
            continue;
        };
        let exec = Execution {
            mode: RouteMode::Direct,
            target,
            wire: Wire::Prop(Box::new(t)),
        };
        if let Ok(v) = v2_execution::option(q, i, e.clone(), payer, exec, head).await {
            out.push(v)
        }
    }
    out
}
