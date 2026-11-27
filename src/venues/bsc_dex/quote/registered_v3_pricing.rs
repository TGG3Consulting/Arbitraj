//! Одиночная нога по стандартному V3. Сравнение чистого результата штатного Router и шага V5.
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
        exec::{executor, registered_v3_wallet::Market},
        pools::{family_variant::Variant, registered_router},
        wallet::signer,
    },
};
use alloy_primitives::{Bytes, aliases::U24};

pub(super) async fn candidates(q: &Quoting, i: &Input, head: Option<Head>) -> Vec<OptionQuote> {
    let ctx = &q.ctx;
    let Ok(payer) = signer::address(ctx) else {
        return Vec::new();
    };
    let executor = if ctx.text("dex.router_mode").is_ok_and(|s| s == "graph")
        && !crate::gateway::quote_stage::settlement()
    {
        executor::address(ctx).ok().flatten()
    } else {
        None
    };
    let (input, output) = if i.side == Side::Buy {
        (i.usdt, i.coin)
    } else {
        (i.coin, i.usdt)
    };
    let entries = i
        .entries
        .iter()
        .filter(|e| e.family.variant == Variant::RegisteredUniV3);
    let mut options = Vec::new();
    for e in entries {
        let pool = e.pool.reference.address();
        let router = if ctx.chain_id() == Some(1) && registered_router::eligible(input, output) {
            match head {
                Some(h) => registered_router::checked(ctx, e, input, output, &h.selector())
                    .await
                    .ok(),
                None => ctx.dex_addr("dex.router", e.family.id).ok(),
            }
        } else {
            None
        };
        if let (Some(target), Some(fee)) = (router, e.pool.fee_pips) {
            let market = Market {
                pool,
                fee: U24::from(fee),
            };
            let order = Execution {
                mode: RouteMode::Direct,
                target,
                wire: Wire::RegisteredV3(market),
            };
            if let Ok(option) = v2_execution::option(q, i, e.clone(), payer, order, head).await {
                options.push(option);
            }
        }
        if let Some(target) = executor {
            let wire = Wire::V5(V5Step {
                kind: 1,
                pool,
                data: Bytes::new(),
                native: None,
            });
            let order = Execution {
                mode: RouteMode::Graph,
                target,
                wire,
            };
            if let Ok(option) = v2_execution::option(q, i, e.clone(), payer, order, head).await {
                options.push(option);
            }
        }
    }
    options
}
