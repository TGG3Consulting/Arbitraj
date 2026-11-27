//! Сравнивает прямой вызов проверенного модуля рынка и шаг V5 по чистой выплате и расходам текущего кошелька.
use super::super::{
    exec::executor,
    pools::{family_variant::Variant, memebox, registered_v3, registry::Entry},
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
    super::super::exec::antfarm_wallet::enabled(e) || super::super::exec::temple_wallet::enabled(e) || !e.family.variant.umbrae() && e.family.variant.wallet_module() && e.family.tradable && e.family.executor_kind.is_some()
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
        if super::super::exec::antfarm_wallet::enabled(e){
            quotes.extend(Box::pin(super::antfarm_pricing::options(q,i,e,head)).await);
            continue;
        }
        let Ok(module) = ctx.dex_addr("dex.router", e.family.id) else {
            continue;
        };
        let mut e = e.clone();
        if let Some(h) = head {
            let check = if e.family.variant==Variant::Temple {
                super::super::pools::temple::market(ctx,e.family,e.pool.reference.address(),(e.coin,e.usdt),&h.selector()).await.map(|s|e.pool.state=crate::core::types::PoolState::Temple(Box::new(s)))
            } else if e.family.variant == Variant::BunnySwap {
                super::super::pools::bunnyswap::fresh(ctx,&e,&h.selector()).await
            } else if e.family.variant == Variant::CandySwap {
                super::super::pools::v2_live_fee::candy_fresh(ctx,&e,&h.selector()).await
            } else if e.family.variant == Variant::RegisteredV3 {
                registered_v3::fresh(ctx, &e, &h.selector()).await
            } else {
                memebox::refresh(ctx, &mut e, h).await
            };
            if check.is_err() {
                continue;
            }
        }
        let direct = Execution {
            mode: RouteMode::Direct,
            target: module,
            wire: if let crate::core::types::PoolState::Temple(s)=&e.pool.state{
                Wire::Temple(std::sync::Arc::new(super::super::exec::temple_wallet::Key::new(s,module)))
            }else{Wire::DirectModule{pool:e.pool.reference.address()}},
        };
        if let Ok(option) = v2_execution::option(q, i, e.clone(), payer, direct, head).await {
            quotes.push(option);
        }
        if graph && let Ok(Some(target)) = executor::address(ctx) {
            let (kind, data) = if e.family.variant == Variant::RegisteredV3 {
                (
                    17,
                    Bytes::copy_from_slice(e.pool.reference.address().into_word().as_slice()),
                )
            } else {
                (29, Bytes::new())
            };
            let step = V5Step {
                native: None,
                kind,
                pool: module,
                data,
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
