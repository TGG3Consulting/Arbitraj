//! Mint Club V2, MCV2_Bond.getReserveForToken/getRefundForTokens.
//! Each step rounds separately. Royalty rounds once on their sum.
#[cfg(test)]
#[path = "mint_club_tests.rs"]
mod tests;
use super::full_math::{mul_div, mul_div_up};
use super::swap_cl::Swapped;
use crate::core::types::MintClubState;
use alloy_primitives::{Address, U256};

pub(super) const UNIT: u64 = 1_000_000_000_000_000_000;
const BPS: u16 = 10_000;

pub(super) fn spans(s: &MintClubState, mint: bool) -> impl Iterator<Item = (U256, U256)> + '_ {
    let start = s.steps.partition_point(|&(end, _)| {
        if mint {
            U256::from(end) <= s.supply
        } else {
            U256::from(end) < s.supply
        }
    });
    let count = if mint {
        s.steps.len() - start
    } else {
        (start + 1).min(s.steps.len())
    };
    (0..count).filter_map(move |offset| {
        let i = if mint {
            start + offset
        } else {
            count - offset - 1
        };
        let lower = i.checked_sub(1).map_or(0, |n| s.steps[n].0);
        let (upper, price) = s.steps[i];
        let (lo, hi) = (U256::from(lower), U256::from(upper));
        let span = if mint {
            hi.checked_sub(lo.max(s.supply))?
        } else {
            hi.min(s.supply).checked_sub(lo)?
        };
        (!span.is_zero()).then_some((span, U256::from(price)))
    })
}

fn subtotal(units: U256, price: U256, mint: bool) -> Option<U256> {
    let product = units.checked_mul(price)?;
    let scale = U256::from(UNIT);
    Some(if mint {
        product.div_ceil(scale)
    } else {
        product / scale
    })
}

fn royalty(gross: U256, rate: u16) -> Option<U256> {
    mul_div(gross, U256::from(rate), U256::from(BPS))
}

pub(super) fn gross_for_refund(net: U256, rate: u16) -> Option<U256> {
    let keep = U256::from(BPS.checked_sub(rate)?);
    mul_div(net.checked_sub(U256::ONE)?, U256::from(BPS), keep)?.checked_add(U256::ONE)
}

fn budget_before_royalty(budget: U256, rate: u16) -> Option<U256> {
    let first = mul_div(
        budget,
        U256::from(BPS),
        U256::from(u32::from(BPS) + u32::from(rate)),
    )?;
    if let Some(next) = first.checked_add(U256::ONE)
        && next
            .checked_add(royalty(next, rate)?)
            .is_some_and(|total| total <= budget)
    {
        return Some(next);
    }
    Some(first)
}

pub struct Fill {
    pub swapped: Swapped,
    pub gross: U256,
    pub royalty: U256,
}

/// The selector takes a token quantity for both mint and burn. For reserve-denominated
/// requests, invert each monotone step, then settle the actual quantity by the same formula.
pub fn fill(s: &MintClubState, input: Address, exact_in: bool, amount: U256) -> Option<Fill> {
    if amount.is_zero() || (input != s.reserve && input != s.token) {
        return None;
    }
    let mint = input == s.reserve;
    let rate = if mint { s.mint_bps } else { s.burn_bps };
    if rate >= BPS {
        return None;
    }
    let in_tokens = mint != exact_in;
    let mut left = if in_tokens {
        amount
    } else if mint {
        budget_before_royalty(amount, rate)?
    } else {
        gross_for_refund(amount, rate)?
    };
    let mut gross = U256::ZERO;
    let mut units = U256::ZERO;
    for (span, price) in spans(s, mint) {
        let full = subtotal(span, price, mint)?;
        let take = if in_tokens {
            left.min(span)
        } else if mint && full > left {
            mul_div(left, U256::from(UNIT), price)?
        } else if !mint && full >= left {
            mul_div_up(left, U256::from(UNIT), price)?
        } else {
            span
        };
        let share = subtotal(take, price, mint)?;
        units = units.checked_add(take)?;
        gross = gross.checked_add(share)?;
        left = left.saturating_sub(if in_tokens { take } else { share });
        if left.is_zero() || take < span {
            break;
        }
    }
    if gross.is_zero() || units.is_zero() || (!left.is_zero() && (in_tokens || !mint)) {
        return None;
    }
    let fee = royalty(gross, rate)?;
    let (paid, received) = if mint {
        (gross.checked_add(fee)?, units)
    } else {
        let net = gross.checked_sub(fee)?;
        if gross > s.bonded || net > s.available || net.is_zero() {
            return None;
        }
        (units, net)
    };
    Some(Fill {
        swapped: Swapped {
            amount_in: paid,
            amount_out: received,
            complete: !exact_in || paid == amount,
        },
        gross,
        royalty: fee,
    })
}

pub fn after(s: &MintClubState, input: Address, amount: U256) -> Option<(Swapped, MintClubState)> {
    let result = fill(s, input, true, amount)?;
    let swapped = result.swapped;
    let mut next = s.clone();
    if input == s.reserve {
        next.supply = next.supply.checked_add(swapped.amount_out)?;
        next.bonded = next.bonded.checked_add(result.gross)?;
        next.available = next.available.checked_add(swapped.amount_in)?;
    } else {
        next.supply = next.supply.checked_sub(swapped.amount_in)?;
        next.bonded = next.bonded.checked_sub(result.gross)?;
        next.available = next.available.checked_sub(swapped.amount_out)?;
    }
    Some((swapped, next))
}
