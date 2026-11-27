//! Родной Bond как самостоятельная прямая котировка. В финале сравниваются тот же выпуск, фактический дебет и газ владельца.
use super::super::{
    exec::{
        mint_club_binding as binding, mint_club_direct as direct, mint_club_wire::Protection,
        native,
    },
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

pub(super) async fn candidates(q: &Quoting, i: &Input, head: Option<Head>) -> Vec<OptionQuote> {
    let mut options = Vec::new();
    if native::is_native(&q.ctx, &i.asset) || native::cash(&q.ctx, &i.usdt_ticker, i.usdt) {
        return options;
    }
    let Ok(payer) = signer::address(&q.ctx) else {
        return options;
    };
    let generation = q.ctx.settings.generation();
    let input = if i.side == Side::Buy { i.usdt } else { i.coin };
    for e in &i.entries {
        if !direct::enabled(e) {
            continue;
        }
        let mut e = e.clone();
        if let Some(h) = head {
            let Ok(s) = direct::state(&q.ctx, &e, h).await else {
                continue;
            };
            e.pool.state = PoolState::MintClub(Box::new(s));
        }
        let PoolState::MintClub(s) = &e.pool.state else {
            continue;
        };
        let Ok((candidate, _)) = super::direct::best(
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
        if candidate.coins != i.qty {
            continue;
        }
        let swap = candidate.swapped;
        let mint = input == s.reserve;
        let qty = if mint {
            swap.amount_out
        } else {
            swap.amount_in
        };
        let minimum = if i.side == Side::Buy {
            let Some(m) = super::direct::ceil_units(i.qty, i.coin_dec) else {
                continue;
            };
            m
        } else {
            alloy_primitives::U256::ONE
        };
        let Some((call, exact)) = direct::call(s, input, qty, swap.amount_in, minimum) else {
            continue;
        };
        let Ok(bond) = q.ctx.dex_addr("dex.factory", e.family.id) else {
            continue;
        };
        if let Some(h) = head
            && direct::check(&q.ctx, bond, call, &exact, h).await.is_err()
        {
            continue;
        }
        let native = match head {
            Some(h) => binding::nominal(&q.ctx, &e, s.reserve, h).await.is_ok(),
            None => {
                q.ctx.addr("dex.native_wrapped").ok() == Some(s.reserve)
                    && q.ctx
                        .dex_list("dex.mint_nominal_reserve_hashes", e.family.id)
                        .is_ok_and(|v| !v.is_empty())
            }
        };
        let mut routes = Vec::new();
        if native {
            routes.push((bond, call));
        }
        if let Ok(module) = q.ctx.dex_addr("dex.router", e.family.id) {
            let protection = match head {
                Some(h) => binding::module(&q.ctx, &e, module, h).await.ok(),
                None if super::super::pools::direct_module::configured(&q.ctx, e.family.id)
                    .is_ok() =>
                {
                    Some(Protection {
                        bond,
                        module,
                        deadline: alloy_primitives::U256::ZERO,
                    })
                }
                None => None,
            };
            if protection.is_some() {
                routes.push((
                    module,
                    super::super::exec::mint_club_wire::Call { protection, ..call },
                ));
            }
        }
        for (target, order) in routes {
            let execution = Execution {
                mode: RouteMode::Direct,
                target,
                wire: Wire::MintClub(order),
            };
            if let Ok(choice) = Box::pin(v2_execution::option(
                q,
                i,
                e.clone(),
                payer,
                execution,
                head,
            ))
            .await
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
