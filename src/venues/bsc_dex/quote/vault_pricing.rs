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

pub(super) fn enabled(e: &Entry) -> bool {
    vault_direct::enabled(e)
        || e.family.variant == super::super::pools::family_variant::Variant::PuzzleRanges
}

async fn forms(
    q: &Quoting,
    i: &Input,
    e: &Entry,
    h: Option<Head>,
) -> Option<(Address, V5Step, Option<Execution>)> {
    use super::super::{
        exec::ranges_wallet,
        pools::{exact_wallet_tokens, family_variant::Variant},
    };
    let args = if i.side == crate::core::types::Side::Buy {
        (i.usdt, i.coin)
    } else {
        (i.coin, i.usdt)
    };
    if e.family.variant == Variant::PuzzleRanges {
        let (m, router) = ranges_wallet::Market::read(&q.ctx, e).ok()?;
        let direct = if exact_wallet_tokens::eligible(&q.ctx, e.family.id, args.0, args.1)
            && match h {
                Some(h) => m
                    .checked(&q.ctx, e, router, args, &h.selector())
                    .await
                    .is_ok(),
                None => true,
            } {
            Some(Execution {
                mode: RouteMode::Direct,
                target: router,
                wire: Wire::Ranges(m),
            })
        } else {
            None
        };
        return Some((
            m.vault,
            V5Step {
                native: None,
                kind: 23,
                pool: m.vault,
                data: m.pool.abi_encode().into(),
            },
            direct,
        ));
    }
    let (vault, id) = vault_direct::market(&q.ctx, e).ok()?;
    if let Some(head) = h {
        vault_direct::checked(&q.ctx, e, head).await.ok()?;
    }
    let direct = vault_direct::exact_output(&q.ctx, e, args.1, h)
        .await
        .is_ok()
        .then_some(Execution {
            mode: RouteMode::Direct,
            target: vault,
            wire: Wire::Vault(id),
        });
    Some((
        vault,
        V5Step {
            native: None,
            kind: 17,
            pool: vault,
            data: id.abi_encode().into(),
        },
        direct,
    ))
}

pub(super) async fn candidates(q: &Quoting, i: &Input, h: Option<Head>) -> Vec<OptionQuote> {
    let Ok(payer) = signer::address(&q.ctx) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for e in i.entries.iter().filter(|e| enabled(e)) {
        let Some((vault, step, direct)) = forms(q, i, e, h).await else {
            continue;
        };
        if let Some(form) = direct
            && let Ok(opt) = Box::pin(v2_execution::option(q, i, e.clone(), payer, form, h)).await
        {
            result.push(opt);
        }
        if (crate::gateway::quote_stage::settlement() && vault_direct::enabled(e))
            || q.ctx.text("dex.router_mode").ok().as_deref() != Some("graph")
        {
            continue;
        }
        let Ok(Some(target)) = executor::address(&q.ctx) else {
            continue;
        };
        let graph = Execution {
            mode: RouteMode::Graph,
            target,
            wire: Wire::V5Funded(step, vec![vault]),
        };
        if let Ok(opt) = Box::pin(v2_execution::option(q, i, e.clone(), payer, graph, h)).await {
            result.push(opt);
        }
    }
    result
}
