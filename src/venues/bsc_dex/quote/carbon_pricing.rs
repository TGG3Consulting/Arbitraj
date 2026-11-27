//! Официальное распределение Carbon по каноническому блоку и замер всего платежа кошелька.
use super::super::{
    exec::{carbon_direct as direct, native},
    pools::carbon_quote,
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
use std::sync::Arc;
pub(super) async fn candidates(q: &Quoting, i: &Input, head: Option<Head>) -> Vec<OptionQuote> {
    let Ok(payer) = signer::address(&q.ctx) else {
        return vec![];
    };
    let native =
        native::is_native(&q.ctx, &i.asset) || native::cash(&q.ctx, &i.usdt_ticker, i.usdt);
    let buy = i.side == Side::Buy;
    let input = if buy { i.usdt } else { i.coin };
    let generation = q.ctx.settings.generation();
    let mut options = Vec::new();
    for original in &i.entries {
        if !direct::enabled(original) {
            continue;
        }
        let mut e = original.clone();
        if let Some(h) = head {
            let Ok(s) = direct::state(&q.ctx, &e, h).await else {
                continue;
            };
            e.pool.state = PoolState::Carbon(Box::new(s));
        }
        let PoolState::Carbon(s) = &e.pool.state else {
            continue;
        };
        if s.native_side.is_some() != native {
            continue;
        }
        let Ok((c, _)) = super::direct::best(
            q,
            &i.asset,
            i.side,
            i.qty,
            vec![e.clone()],
            i.coin_dec,
            i.usdt_dec,
        ) else {
            continue;
        };
        if c.coins != i.qty {
            continue;
        }
        let sw = c.swapped;
        let size = if buy { sw.amount_out } else { sw.amount_in };
        let min = if buy { sw.amount_out } else { U256::ONE };
        let Ok((order, p)) = direct::call(
            &q.ctx,
            s,
            input,
            buy,
            size,
            sw.amount_in,
            min,
            head.map_or(0, |h| h.timestamp.to()),
        ) else {
            continue;
        };

        if let Some(h) = head
            && carbon_quote::official(&q.ctx, s, input, &p, &h.selector())
                .await
                .is_err()
        {
            continue;
        }
        for protected in [false, true] {
            let mut signed = order.clone();
            if let Some(h) = head {
                if direct::bound(&q.ctx, &e, &mut signed, h, protected)
                    .await
                    .is_err()
                {
                    continue;
                }
            } else if protected {
                let Ok(m) = q.ctx.dex_addr("dex.router", e.family.id) else {
                    continue;
                };
                if super::super::pools::direct_module::configured(&q.ctx, e.family.id).is_err() {
                    continue;
                }
                signed.module = Some(m);
            } else if signed.target != super::super::pools::carbon::NATIVE {
                continue;
            }
            let exec = Execution {
                mode: RouteMode::Direct,
                target: signed.recipient(),
                wire: Wire::Carbon(Arc::new(signed)),
            };
            if let Ok(choice) =
                Box::pin(v2_execution::option(q, i, e.clone(), payer, exec, head)).await
            {
                options.push(choice);
            }
        }
    }
    if q.ctx.settings.generation() != generation {
        options.clear();
    }
    options
}
