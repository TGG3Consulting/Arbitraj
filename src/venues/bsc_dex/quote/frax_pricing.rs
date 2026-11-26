//! Для одного пула Fraxswap моделирует штатный роутер и V5 с адаптером при одном входе и блоке.
//! Сравнение чистой стоимости с разрешениями выполняет общий v2_execution.
use super::super::{
    ctx::Ctx,
    exec::executor,
    pools::{family_variant::Variant, fraxswap, registry::Entry},
    wallet::signer,
};
use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    v2_wallet_sim::V5Step,
    white_state::Head,
};
use crate::{
    core::types::RouteMode,
    gateway::{errors::GatewayResult, quote_stage},
};
use alloy_primitives::Bytes;

pub(crate) fn is_frax(e: &Entry) -> bool {
    e.family.variant == Variant::FraxTwammRouter
        || (e.family.variant == Variant::FraxTwamm && e.family.executor_kind.is_some())
}

pub(crate) async fn refresh(ctx: &Ctx, e: &mut Entry, h: Head) -> GatewayResult<()> {
    let data = fraxswap::at(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.pool.token0, e.pool.token1),
        &h.selector(),
        h.timestamp,
    )
    .await?;
    e.pool.state = data.state;
    e.pool.block = h.number.to();
    e.pool.fee_pips = data.fee_pips;
    Ok(())
}

pub(super) async fn candidates(q: &Quoting, i: &Input, head: Option<Head>) -> Vec<OptionQuote> {
    let ctx = &q.ctx;
    let Ok(payer) = signer::address(ctx) else {
        return Vec::new();
    };
    let graph_allowed =
        !quote_stage::settlement() && ctx.text("dex.router_mode").ok().as_deref() == Some("graph");
    let mut options = Vec::new();
    for entry in i.entries.iter().filter(|e| is_frax(e)) {
        let mut e = entry.clone();
        if let Some(h) = head
            && refresh(ctx, &mut e, h).await.is_err()
        {
            continue;
        }
        let native = if let Some(h) = head {
            fraxswap::native(ctx, e.family, &h.selector()).await
        } else {
            ctx.dex_text("dex.fraxswap_router_hash", e.family.id)
                .and_then(|_| ctx.dex_addr("dex.quoter", e.family.id))
        };
        if let Ok(target) = native
            && let Ok(quote) = v2_execution::option(
                q,
                i,
                e.clone(),
                payer,
                Execution {
                    mode: RouteMode::Direct,
                    target,
                    wire: super::v2_wallet_sim::Wire::Router02,
                },
                head,
            )
            .await
        {
            options.push(quote);
        }
        if !graph_allowed || e.family.executor_kind.is_none() {
            continue;
        }
        let (Ok(Some(target)), Ok(adapter)) = (
            executor::address(ctx),
            ctx.dex_addr("dex.router", e.family.id),
        ) else {
            continue;
        };
        if let Some(h) = head
            && fraxswap::adapter_at(
                ctx,
                e.family,
                e.pool.reference.address(),
                (e.pool.token0, e.pool.token1),
                &h.selector(),
            )
            .await
            .is_err()
        {
            continue;
        }
        let wire = super::v2_wallet_sim::Wire::V5(V5Step {
            kind: 29,
            pool: adapter,
            data: Bytes::new(),
        });
        if let Ok(quote) = v2_execution::option(
            q,
            i,
            e,
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
            options.push(quote);
        }
    }
    options
}
