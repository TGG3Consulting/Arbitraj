//! Several Temple pairs may spend one router's inventory of the defense token.
use super::super::{ctx::Ctx, math::temple};
use super::{path::Path, route_calc::Leg};
use crate::{
    core::types::PoolState,
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy_primitives::{Address, U256};
use std::collections::{HashMap, HashSet};

fn candidate(leg: &Leg) -> Option<(Address, Address)> {
    let PoolState::Temple(s) = &leg.entry.pool.state else {
        return None;
    };
    (leg.token_in == s.temple && leg.token_out == s.defend).then_some((s.router, s.defend))
}

pub(super) fn shared(a: &Leg, b: &Leg) -> bool {
    candidate(a).is_some_and(|key| candidate(b) == Some(key))
}

struct Debit {
    router: Address,
    token: Address,
    pair: Address,
    balance: U256,
    amount: U256,
}

fn fits(mut withdrawals: impl Iterator<Item = Debit>) -> bool {
    let Some(first) = withdrawals.next() else {
        return true;
    };
    let Some(second) = withdrawals.next() else {
        return first.amount <= first.balance;
    };
    let mut groups = HashMap::new();
    for v in [first, second].into_iter().chain(withdrawals) {
        let (spent, available, seen) = groups
            .entry((v.router, v.token))
            .or_insert_with(|| (U256::ZERO, v.balance, HashSet::new()));
        if seen.insert(v.pair) {
            *available = (*available).min(v.balance);
        }
        let Some(total) = spent.checked_add(v.amount) else {
            return false;
        };
        if total > *available {
            return false;
        }
        *spent = total;
    }
    true
}

pub(super) fn check<'a>(ctx: &Ctx, paths: impl IntoIterator<Item = &'a Path>) -> GatewayResult<()> {
    let uses = paths.into_iter().flat_map(|p| &p.hops).filter_map(|h| {
        let PoolState::Temple(s) = &h.entry.pool.state else {
            return None;
        };
        (h.token_in == s.temple
            && h.token_out == s.defend
            && (s.stable != s.defend
                || temple::is_defense(s, h.token_in, h.swapped.amount_in) == Some(true)))
        .then_some(Debit {
            router: s.router,
            token: s.defend,
            pair: s.pair,
            balance: s.defense_balance,
            amount: h.swapped.amount_out,
        })
    });
    if fits(uses) {
        Ok(())
    } else {
        Err(ctx.err(
            ErrorCode::NoRoute,
            "TempleDAO: сумма выплат по маршруту превышает общий запас этой валюты у роутера",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cash_from_different_pairs_is_counted_once() {
        let use_cash = |pair: u8, available: u64, n: u64| Debit {
            router: Address::repeat_byte(1),
            token: Address::repeat_byte(2),
            pair: Address::repeat_byte(pair),
            balance: U256::from(available),
            amount: U256::from(n),
        };
        assert!(fits(
            [use_cash(3, 10, 6), use_cash(3, 4, 2), use_cash(4, 10, 2)].into_iter()
        ));
        assert!(!fits([use_cash(3, 10, 7), use_cash(4, 10, 4)].into_iter()));
        assert!(!fits([use_cash(3, 10, 5), use_cash(4, 8, 4)].into_iter()));
    }
}
