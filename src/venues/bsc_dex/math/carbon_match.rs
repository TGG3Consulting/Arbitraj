//! Carbon uses the strategy IDs chosen by its taker. A physical strategy appears at most once per transaction.
use super::carbon::{amount, expand, raw, with_fee};
use crate::core::types::{CarbonOrder, CarbonState};
use alloy_primitives::{Address, U256};
use std::cmp::Ordering;
#[cfg(test)]
#[path = "carbon_match_tests.rs"]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub id: U256,
    pub amount: u128,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub actions: Vec<Action>,
    pub spent: u128,
    pub received: u128,
    pub fee: u128,
    pub by_target: bool,
}

struct Slot<'a> {
    id: U256,
    order: &'a CarbonOrder,
    cap: u128,
    other: u128,
    top: U256,
    a: U256,
    b: U256,
}

pub(super) fn capacity(
    o: &CarbonOrder,
    room: u128,
    by_target: bool,
    want: u128,
) -> Option<(u128, u128)> {
    if o.y == 0 || room == 0 || o.z < o.y || o.a == 0 && o.b == 0 {
        return None;
    }
    let high = want.min(if by_target { o.y } else { room });
    let max_other = if by_target { room } else { o.y };
    let within = |size| raw(o, by_target, size).is_some_and(|x| x <= max_other);
    let mut cap = high;
    if !within(high) {
        // The inverse curve usually puts the boundary within a single raw unit.
        let guess = raw(o, !by_target, max_other).unwrap_or(high).min(high);
        let (mut low, mut end) = if within(guess) {
            (guess, high)
        } else {
            (0, guess)
        };
        if low < end && !within(low + 1) {
            end = low;
        }
        while low < end {
            let mid = low + (end - low).div_ceil(2);
            if within(mid) {
                low = mid;
            } else {
                end = mid - 1;
            }
        }
        cap = low;
    }
    Some((cap, amount(o, by_target, cap)?))
}

fn slots(s: &CarbonState, input: Address, by_target: bool, want: u128) -> Option<Vec<Slot<'_>>> {
    let i = s.tokens.iter().position(|a| *a == input)?;
    let mut slots: Vec<_> = s
        .strategies
        .iter()
        .filter_map(|st| {
            let o = &st.orders[1 - i];
            let room = u128::MAX - st.orders[i].y;
            let (cap, other) = capacity(o, room, by_target, want)?;
            let a = expand(o.a)?;
            let b = expand(o.b)?;
            let top = U256::from(o.y) * a / U256::from(o.z) + b;
            Some(Slot {
                id: st.id,
                order: o,
                cap,
                other,
                top,
                a,
                b,
            })
        })
        .collect();
    slots.sort_unstable_by(|x, y| {
        let left = U256::from(x.other) * U256::from(y.cap);
        let right = U256::from(y.other) * U256::from(x.cap);
        (if by_target {
            left.cmp(&right)
        } else {
            right.cmp(&left)
        })
        .then_with(|| y.cap.cmp(&x.cap))
        .then_with(|| x.id.cmp(&y.id))
    });
    Some(slots)
}

impl Slot<'_> {
    fn at_rate(&self, r: U256, by_target: bool) -> u128 {
        if r >= self.top {
            return 0;
        }
        if self.a.is_zero() || r <= self.b {
            return self.cap;
        }
        // (y*A + z*B - z*r)/A, clipped before converting to source currency.
        let y = U256::from(self.order.y);
        let target = (y * self.a - U256::from(self.order.z) * (r - self.b)) / self.a;
        let n = target.to::<u128>();
        if by_target {
            n.min(self.cap)
        } else {
            raw(self.order, true, n).unwrap_or(self.cap).min(self.cap)
        }
    }
}

fn assemble(
    slots: &[Slot],
    mut sizes: Vec<u128>,
    s: &CarbonState,
    target: bool,
    want: u128,
) -> Option<Plan> {
    let mut used = sizes.iter().try_fold(0u128, |sum, &n| sum.checked_add(n))?;
    if used > want {
        return None;
    }
    // Fill raw-unit residue after equalizing the marginal rates. The sorted order is also
    // a separate whole-size allocation candidate when discreteness dominates the curve.
    for (slot, n) in slots.iter().zip(&mut sizes) {
        let add = (want - used).min(slot.cap - *n);
        if add != 0 && amount(slot.order, target, *n + add).is_some() {
            *n += add;
            used += add;
        }
        if used == want {
            break;
        }
    }
    if used != want {
        return None;
    }
    let mut actions = Vec::new();
    let mut gross = 0u128;
    for (slot, n) in slots.iter().zip(sizes) {
        if n == 0 {
            continue;
        }
        gross = gross.checked_add(amount(slot.order, target, n)?)?;
        actions.push(Action {
            id: slot.id,
            amount: n,
        });
    }
    let total = with_fee(gross, target, s.fee_ppm)?;
    if total == 0 {
        return None;
    }
    let (spent, received) = if target { (total, want) } else { (want, total) };
    Some(Plan {
        actions,
        spent,
        received,
        fee: total.abs_diff(gross),
        by_target: target,
    })
}

fn cmp(a: &Plan, b: &Plan) -> Ordering {
    (if a.by_target {
        a.spent.cmp(&b.spent)
    } else {
        b.received.cmp(&a.received)
    })
    .then_with(|| a.actions.len().cmp(&b.actions.len()))
}

/// Preserve the best single, depth-first and marginal-rate allocations for gas-aware selection.
pub fn alternatives(s: &CarbonState, input: Address, by_target: bool, want: u128) -> Vec<Plan> {
    if want == 0 {
        return vec![];
    }
    let Some(slots) = slots(s, input, by_target, want) else {
        return vec![];
    };
    if slots.is_empty() {
        return vec![];
    }
    let mut plans = Vec::new();
    if let Some(i) = slots.iter().position(|x| x.cap == want) {
        let mut n = vec![0; slots.len()];
        n[i] = want;
        plans.extend(assemble(&slots, n, s, by_target, want));
    }
    plans.extend(assemble(&slots, vec![0; slots.len()], s, by_target, want));
    if slots.len() > 1 && slots.iter().any(|x| !x.a.is_zero()) {
        let mut lo = U256::ZERO;
        let mut hi = slots.iter().map(|x| x.top).max().unwrap_or_default();
        while lo < hi {
            let r = lo + (hi - lo) / U256::from(2);
            // Keep the order allocation within the requested input even at u128::MAX.
            let enough = slots
                .iter()
                .try_fold(0u128, |sum, x| sum.checked_add(x.at_rate(r, by_target)));
            if enough.is_none_or(|sum| sum > want) {
                lo = r + U256::ONE;
            } else {
                hi = r;
            }
        }
        plans.extend(assemble(
            &slots,
            slots.iter().map(|x| x.at_rate(lo, by_target)).collect(),
            s,
            by_target,
            want,
        ));
    }
    plans.sort_by(cmp);
    plans.dedup();
    plans
}

pub fn plan(s: &CarbonState, input: Address, by_target: bool, want: u128) -> Option<Plan> {
    alternatives(s, input, by_target, want).into_iter().next()
}

pub fn after(s: &CarbonState, input: Address, fill: &Plan) -> Option<CarbonState> {
    let i = s.tokens.iter().position(|t| *t == input)?;
    let mut next = s.clone();
    for action in &fill.actions {
        let r = next.strategies.iter_mut().find(|r| r.id == action.id)?;
        let n = amount(&r.orders[1 - i], fill.by_target, action.amount)?;
        let (credit, debit) = if fill.by_target {
            (n, action.amount)
        } else {
            (action.amount, n)
        };
        r.orders[1 - i].y = r.orders[1 - i].y.checked_sub(debit)?;
        let source = &mut r.orders[i];
        source.y = source.y.checked_add(credit)?;
        source.z = source.z.max(source.y);
    }
    Some(next)
}
