//! Сопоставляет отдельный ERC-20 обмен через LBRouter и V5 с теми же балансами и газом.
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
        exec::{executor, sectorone::Path},
        pools::sectorone,
        wallet::signer,
    },
};
use alloy_primitives::Bytes;
pub(super) async fn candidates(q: &Quoting, i: &Input, head: Option<Head>) -> Vec<OptionQuote> {
    let ctx = &q.ctx;
    let Ok(payer) = signer::address(ctx) else {
        return Vec::new();
    };
    let graph = ctx.text("dex.router_mode").is_ok_and(|s| s == "graph");
    let mut quotes = Vec::new();
    for e in i.entries.iter().filter(|e| sectorone::enabled(e)) {
        if e.family.variant.e3_lb() && let Some(h) = head {
            let input=if i.side==crate::core::types::Side::Buy{i.usdt}else{i.coin};
            if crate::venues::bsc_dex::pools::e3::input(ctx,e.pool.reference.address(),input,&h.selector()).await.is_err(){continue;}
        }
        let Some((legacy, step)) = sectorone::bin(e) else {
            continue;
        };
        if let Ok(target) = ctx.dex_addr("dex.router", e.family.id) {
            let valid = match head {
                Some(h) => sectorone::check(ctx, e, &h.selector(), true).await.is_ok(),
                None => true,
            };
            if valid {
                let order = Execution {
                    target,
                    mode: RouteMode::Direct,
                    wire: Wire::SectorLb(Path { legacy, step }),
                };
                if let Ok(x) = v2_execution::option(q, i, e.clone(), payer, order, head).await {
                    quotes.push(x);
                }
            }
        }
        if graph && let Ok(Some(target)) = executor::address(ctx) {
            if let Some(h) = head
                && sectorone::check(ctx, e, &h.selector(), false)
                    .await
                    .is_err()
            {
                continue;
            }
            let wire = Wire::V5(V5Step {
                native: None,
                kind: if legacy { 24 } else { 18 },
                pool: e.pool.reference.address(),
                data: Bytes::new(),
            });
            let order = Execution {
                target,
                mode: RouteMode::Graph,
                wire,
            };
            if let Ok(x) = v2_execution::option(q, i, e.clone(), payer, order, head).await {
                quotes.push(x);
            }
        }
    }
    quotes
}
