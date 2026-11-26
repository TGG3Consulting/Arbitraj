//! Compare the real wallet payout and gas for the Vault and its executor on one parent block.
use super::super::{
    ctx::Ctx,
    exec::{executor, vault_direct},
    pools::registry::Entry,
    wallet::signer,
};
use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    v2_wallet_sim::{self, Order, V5Step, Wire},
    white_state::Head,
};
use crate::{core::types::RouteMode, gateway::errors::GatewayResult};
use alloy::sol_types::SolValue;
use alloy_primitives::Address;

pub(in crate::venues::bsc_dex) async fn confirm(
    ctx: &Ctx,
    h: Head,
    entry: &Entry,
    payer: Address,
    order: &super::super::exec::plan::Order,
) -> GatewayResult<()> {
    let (target, id) = vault_direct::market(ctx, entry)?;
    Box::pin(v2_wallet_sim::measure(
        ctx,
        h,
        Order {
            payer,
            target,
            input: order.token_in,
            output: order.token_out,
            amount: order.amount_in,
            minimum: order.min_out,
            wire: Wire::Vault(id),
        },
        true,
    ))
    .await?;
    Ok(())
}

pub(super) async fn candidates(q: &Quoting, i: &Input, h: Option<Head>) -> Vec<OptionQuote> {
    let Ok(payer) = signer::address(&q.ctx) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for e in i.entries.iter().filter(|e| vault_direct::enabled(e)) {
        let Ok((vault, id)) = vault_direct::market(&q.ctx, e) else {
            continue;
        };
        if let Some(head) = h
            && vault_direct::checked(&q.ctx, e, head).await.is_err()
        {
            continue;
        }
        let direct = Execution {
            mode: RouteMode::Direct,
            target: vault,
            wire: Wire::Vault(id),
        };
        let output = if i.side == crate::core::types::Side::Buy {
            i.coin
        } else {
            i.usdt
        };
        if vault_direct::exact_output(&q.ctx, e, output, h)
            .await
            .is_ok()
            && let Ok(opt) = Box::pin(v2_execution::option(q, i, e.clone(), payer, direct, h)).await
        {
            result.push(opt);
        }
        if crate::gateway::quote_stage::settlement()
            || q.ctx.text("dex.router_mode").ok().as_deref() != Some("graph")
        {
            continue;
        }
        let Ok(Some(target)) = executor::address(&q.ctx) else {
            continue;
        };
        let step = V5Step {
            native: None,
            kind: 17,
            pool: vault,
            data: id.abi_encode().into(),
        };
        let graph = Execution {
            mode: RouteMode::Graph,
            target,
            wire: Wire::V5(step),
        };
        if let Ok(opt) = Box::pin(v2_execution::option(q, i, e.clone(), payer, graph, h)).await {
            result.push(opt);
        }
    }
    result
}
