//! Размер обычной пробы и глубина ступенчатого ERC20-рынка в полосе маржинальной цены.
use super::full_math::{mul_div, mul_div_up};
use super::mint_club::{UNIT, gross_for_refund, spans};
use crate::core::types::MintClubState;
use alloy_primitives::{Address, U256};

pub fn spot(s: &MintClubState, coin: Address, amount: U256) -> Option<U256> {
    // Sizing a sell probe must still work when the next mint is sold out.
    let mint = coin == s.reserve;
    let p = spans(s, mint)
        .chain(spans(s, !mint))
        .find(|(_, p)| !p.is_zero())?
        .1;
    if mint {
        mul_div(amount, p, U256::from(UNIT))
    } else {
        mul_div(amount, U256::from(UNIT), p)
    }
}

pub fn band(s: &MintClubState, input: Address, buy: bool, band: u64) -> Option<(U256, bool)> {
    let unit = U256::from(UNIT);
    let width = U256::from(band);
    let mint = input == s.reserve;
    // Inverse price when the tracked coin is itself the reserve asset.
    let (num, den) = match (mint, buy) {
        (true, true) => (unit + width, unit),
        (true, false) => (unit, unit.checked_sub(width)?),
        (false, false) => (unit - width, unit),
        (false, true) => (unit, unit + width),
    };
    let gross_cap = if s.available >= s.bonded || mint {
        s.bonded
    } else {
        s.bonded.min(
            gross_for_refund(s.available.checked_add(U256::ONE)?, s.burn_bps)?
                .checked_sub(U256::ONE)?,
        )
    };
    let mut base = None;
    let mut tokens = U256::ZERO;
    let mut gross = U256::ZERO;
    for (mut span, price) in spans(s, mint) {
        // A zero-price prefix can only mint together with a paid step.
        if price.is_zero() && !mint {
            break;
        }
        if !price.is_zero() {
            let reference = *base.get_or_insert(price);
            let lhs = price.checked_mul(den)?;
            let rhs = reference.checked_mul(num)?;
            if (mint && lhs > rhs) || (!mint && lhs < rhs) {
                break;
            }
        }
        let step = if mint {
            mul_div_up(span, price, unit)?
        } else {
            mul_div(span, price, unit)?
        };
        let mut cost = step;
        if !mint && step > gross_cap.checked_sub(gross)? {
            // Last token quantity whose floor-valued refund still fits the remaining backing.
            let remain = gross_cap - gross;
            span =
                mul_div_up(remain.checked_add(U256::ONE)?, unit, price)?.checked_sub(U256::ONE)?;
            cost = mul_div(span, price, unit)?;
        }
        tokens = tokens.checked_add(span)?;
        gross = gross.checked_add(cost)?;
        if cost != step {
            break;
        }
    }
    if gross.is_zero() {
        return Some((U256::ZERO, true));
    }
    // Depth is expressed before royalty. Actual executable quotes separately charge it.
    Some((if buy == mint { gross } else { tokens }, true))
}
