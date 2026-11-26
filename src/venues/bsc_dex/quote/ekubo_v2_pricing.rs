//! Official Ekubo V2 Router. Native-key pools are admitted only by a sender path that accounts for ETH.
use super::super::{
    pools::{ekubo_v2, family_variant::Variant, registry::Entry},
    wallet::signer,
};
use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    v2_wallet_sim::Wire,
    white_state::Head,
};
use crate::core::types::{PoolState, RouteMode, Side};
use alloy_primitives::U256;

pub(in crate::venues::bsc_dex) fn enabled(e: &Entry) -> bool {
    e.family.variant == Variant::EkuboV2
}

pub(super) async fn candidates(q: &Quoting, i: &Input, head: Option<Head>) -> Vec<OptionQuote> {
    let Ok(payer) = signer::address(&q.ctx) else {
        return Vec::new();
    };
    let input = if i.side == Side::Buy { i.usdt } else { i.coin };
    let mut choices = Vec::new();
    for e in &i.entries {
        if !enabled(e) || !matches!(&e.pool.state,PoolState::Ekubo(d) if d.native.is_none()) {
            continue;
        }
        let Some(key) = ekubo_v2::order(e, input, U256::ONE) else {
            continue;
        };
        let target = match head {
            Some(h) => {
                ekubo_v2::check(&q.ctx, e.family, e.pool.reference.address(), &h.selector()).await
            }
            None => q.ctx.dex_addr("dex.quoter", e.family.id),
        };
        let Ok(target) = target else {
            continue;
        };
        let exec = Execution {
            mode: RouteMode::Direct,
            target,
            wire: Wire::EkuboV2(key),
        };
        if let Ok(choice) = Box::pin(v2_execution::option(q, i, e.clone(), payer, exec, head)).await
        {
            choices.push(choice);
        }
    }
    choices
}
