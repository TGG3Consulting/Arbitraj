//! Родной Bond как самостоятельная прямая котировка. В финале сравниваются тот же выпуск, фактический дебет и газ владельца.
use super::super::{
    exec::{mint_club_direct as direct, native},
    math::mint_club as math,
    num,
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
    let (input, amount) = match i.side {
        Side::Buy => (i.usdt, super::direct::ceil_units(i.qty, i.coin_dec)),
        Side::Sell => (i.coin, num::to_units(i.qty, i.coin_dec)),
    };
    let Some(amount) = amount else { return options };
    for e in &i.entries {
        let PoolState::MintClub(cached) = &e.pool.state else {
            continue;
        };
        if q.ctx.addr("dex.native_wrapped").ok() != Some(cached.reserve) {
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
        let Some(f) = math::fill(s, input, i.side == Side::Sell, amount) else {
            continue;
        };
        if !f.swapped.complete {
            continue;
        }
        let mint = input == s.reserve;
        let qty = if mint {
            f.swapped.amount_out
        } else {
            f.swapped.amount_in
        };
        let Some((call, exact)) =
            direct::call(s, input, qty, f.swapped.amount_in, f.swapped.amount_out)
        else {
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
        let execution = Execution {
            mode: RouteMode::Direct,
            target: bond,
            wire: Wire::MintClub(call),
        };
        if let Ok(choice) = Box::pin(v2_execution::option(q, i, e, payer, execution, head)).await {
            options.push(choice);
        }
    }
    if q.ctx.settings.generation() != generation {
        options.clear();
    }
    options
}
