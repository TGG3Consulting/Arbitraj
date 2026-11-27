//! Сравнивает официальный DLMM Router, отдельный модуль и шаг V5 по кошельковому результату.
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
        exec::{executor, umbrae::Path},
        pools::{family_variant::Variant, umbrae},
        wallet::signer,
    },
};
use alloy_primitives::Bytes;

pub(super) async fn candidates(q: &Quoting, i: &Input, head: Option<Head>) -> Vec<OptionQuote> {
    let ctx = &q.ctx;
    let Ok(payer) = signer::address(ctx) else {
        return Vec::new();
    };
    let graph = ctx.text("dex.router_mode").is_ok_and(|s| s == "graph")
        && !crate::gateway::quote_stage::settlement();
    let mut quotes = Vec::new();
    for e in i.entries.iter().filter(|e| e.family.variant.umbrae()) {
        let pool = e.pool.reference.address();
        if let Some(h) = head
            && umbrae::market(ctx, e.family, pool, (e.coin, e.usdt), &h.selector())
                .await
                .is_err()
        {
            continue;
        }
        if e.family.variant == Variant::UmbraeDlmm
            && let Some(step) = e.pool.bin_step.and_then(|s| u16::try_from(s).ok())
            && let Some(target) = umbrae::native(ctx, e.family)
        {
            let execution = Execution {
                mode: RouteMode::Direct,
                target,
                wire: Wire::Umbrae(Path { pool, step }),
            };
            if let Ok(option) = v2_execution::option(q, i, e.clone(), payer, execution, head).await
            {
                quotes.push(option);
            }
        }
        if !e.family.tradable {
            continue;
        }
        let Ok(module) = ctx.dex_addr("dex.router", e.family.id) else {
            continue;
        };
        if let Some(h) = head
            && umbrae::module(ctx, e.family, module, &h.selector())
                .await
                .is_err()
        {
            continue;
        }
        let execution = Execution {
            mode: RouteMode::Direct,
            target: module,
            wire: Wire::DirectModule { pool },
        };
        if let Ok(option) = v2_execution::option(q, i, e.clone(), payer, execution, head).await {
            quotes.push(option);
        }
        if graph && let Ok(Some(target)) = executor::address(ctx) {
            let wire = Wire::V5(V5Step {
                native: None,
                kind: 17,
                pool: module,
                data: Bytes::copy_from_slice(pool.into_word().as_slice()),
            });
            if let Ok(option) = v2_execution::option(
                q,
                i,
                e.clone(),
                payer,
                Execution {
                    mode: RouteMode::Graph,
                    target,
                    wire,
                },
                head,
            )
            .await
            {
                quotes.push(option);
            }
        }
    }
    quotes
}
