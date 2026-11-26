//! Классический EOA-маршрут и V5 сравниваются как два способа исполнения
//! одного пула. Объём, баланс плательщика и валюта затрат у обоих одинаковы.
use super::super::{
    exec::executor,
    pools::{family_variant::Variant, registry::Entry},
    wallet::signer,
};
use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    v2_wallet_sim::V5Step,
    white_state,
};
use crate::core::types::{PoolState, RouteMode};
use crate::gateway::quote_stage;
use alloy::sol_types::SolValue;
use alloy_primitives::{Bytes, U256};

pub(super) fn is_white(e: &Entry) -> bool {
    e.family.variant == Variant::WhiteSwap
}

pub(super) async fn candidates(
    q: &Quoting,
    i: &Input,
    h: Option<white_state::Head>,
) -> Vec<OptionQuote> {
    let Ok(payer) = signer::address(&q.ctx) else {
        return vec![];
    };
    let mut options = vec![];
    for entry in i.entries.iter().filter(|e| is_white(e)) {
        let mut e = entry.clone();
        let pips = if let Some(head) = h {
            let tag = head.selector();
            let Ok((r0, r1)) = super::super::pools::whiteswap::check(
                &q.ctx,
                e.family,
                e.pool.reference.address(),
                (e.pool.token0, e.pool.token1),
                &tag,
            )
            .await
            else {
                continue;
            };
            e.pool.state = PoolState::V2 {
                reserve0: r0,
                reserve1: r1,
            };
            e.pool.block = head.number.to();
            white_state::checked_fee(&q.ctx, &e, payer, &tag).await.ok()
        } else {
            white_state::preliminary(q, &e, payer)
        };
        let allow_executor = !quote_stage::settlement()
            && q.ctx.text("dex.router_mode").ok().as_deref() == Some("graph");
        if allow_executor && let Ok(Some(target)) = executor::address(&q.ctx) {
            let mut base = e.clone();
            base.pool.fee_pips = Some(3000);
            let v5 = V5Step {
                            native:None,
                kind: 0,
                pool: e.pool.reference.address(),
                data: Bytes::from(U256::from(3000).abi_encode()),
            };
            if let Ok(v) = v2_execution::option(
                q,
                i,
                base,
                payer,
                Execution {
                    mode: RouteMode::Graph,
                    target,
                    wire: super::v2_wallet_sim::Wire::V5(v5),
                },
                h,
            )
            .await
            {
                options.push(v);
            }
        }
        if let Some(pips) = pips
            && let Ok(target) = q.ctx.dex_addr("dex.router", e.family.id)
        {
            e.pool.fee_pips = Some(pips);
            if let Ok(v) = v2_execution::option(
                q,
                i,
                e,
                payer,
                Execution {
                    mode: RouteMode::Direct,
                    target,
                    wire: super::v2_wallet_sim::Wire::Router02,
                },
                h,
            )
            .await
            {
                options.push(v);
            }
        }
    }
    options
}
