//! Деление только между разными фактическими пулами. Входной сбор Epsilon округляется вниз один раз.
use super::{profile::State, wire::Plan};
use crate::venues::bsc_dex::{parts::Parts, pools::registry::Entry, quote::pool_quote};
use alloy_primitives::{Address, U256, aliases::U24};
use std::sync::Arc;

pub(super) struct Choice {
    pub plan: Plan,
    pub received: U256,
    pub families: [Option<&'static str>; 2],
}
fn gross(net: U256, fee: u16) -> Option<U256> {
    if net.is_zero() || fee >= 10000 {
        return None;
    }
    (net - U256::ONE)
        .checked_mul(U256::from(10000))?
        .checked_div(U256::from(10000 - fee))?
        .checked_add(U256::ONE)
}
fn part(
    p: &Parts,
    entries: &[Arc<Entry>],
    input: Address,
    buy: bool,
    n: U256,
) -> Option<(Option<Arc<Entry>>, U256)> {
    if n.is_zero() {
        return Some((None, U256::ZERO));
    }
    let mut best = None;
    for e in entries {
        if let Ok(s) = pool_quote::swap(&p.ctx, e, input, !buy, n)
            && s.complete
            && (if buy {
                s.amount_out == n
            } else {
                s.amount_in == n
            })
        {
            let gives = if buy { s.amount_in } else { s.amount_out };
            if gives > U256::ZERO
                && best
                    .as_ref()
                    .is_none_or(|(_, q)| if buy { gives < *q } else { gives > *q })
            {
                best = Some((Some(e.clone()), gives))
            }
        }
    }
    best
}
pub(super) fn choices(
    p: &Parts,
    state: &State,
    input: Address,
    output: Address,
    buy: bool,
    raw: U256,
) -> Vec<Choice> {
    let mut sets = [vec![], vec![]];
    for e in p.registry.pair_arcs(input, output) {
        if !e.valid || e.asleep || e.pool.fee_pips.is_none() {
            continue;
        }
        for (i, cfg) in state.config.providers.iter().enumerate() {
            if e.family.id == cfg.family && state.factories[i].is_some() {
                sets[i].push(e.clone())
            }
        }
    }
    let net = if buy {
        raw
    } else {
        let Some(product) = raw.checked_mul(U256::from(state.fee)) else {
            return vec![];
        };
        raw - product / U256::from(10000)
    };
    let mut result = vec![];
    for bps in [0u64, 2500, 5000, 7500, 10000] {
        let Some(split) = net
            .checked_mul(U256::from(bps))
            .map(|x| x / U256::from(10000))
        else {
            continue;
        };
        let Some((first, x)) = part(p, &sets[0], input, buy, split) else {
            continue;
        };
        let Some((second, y)) = part(p, &sets[1], input, buy, net - split) else {
            continue;
        };
        if first
            .as_ref()
            .zip(second.as_ref())
            .is_some_and(|(a, b)| a.shares_liquidity(b))
        {
            continue;
        }
        let Some(total) = x.checked_add(y) else {
            continue;
        };
        let qty = if buy {
            let Some(v) = gross(total, state.fee) else {
                continue;
            };
            v
        } else {
            raw
        };
        let legs = [first, second];
        let plan = Plan {
            router: state.config.router.address,
            providers: state
                .config
                .providers
                .each_ref()
                .map(|c| c.contract.address),
            pools: legs.each_ref().map(|x| {
                x.as_ref()
                    .map_or(Address::ZERO, |e| e.pool.reference.address())
            }),
            tiers: legs
                .each_ref()
                .map(|x| U24::from(x.as_ref().and_then(|e| e.pool.fee_pips).unwrap_or_default())),
            input,
            output,
            amount: qty,
            first: if buy { x } else { split },
            fee_bps: state.fee,
            minimum: if buy { raw } else { U256::ONE },
            deadline: 0,
            wallet: None,
        };
        if result.iter().any(|c: &Choice| {
            c.plan.first == plan.first && c.plan.pools == plan.pools && c.plan.amount == plan.amount
        }) {
            continue;
        }
        result.push(Choice {
            plan,
            received: if buy { raw } else { total },
            families: legs.each_ref().map(|x| x.as_ref().map(|e| e.family.id)),
        });
    }
    result
}
