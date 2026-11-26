//! FRouter/FRouterV2: kLast и виртуальные резервы. Продажа удерживает налог из выхода VIRTUAL.
use super::swap_cl::Swapped;
use crate::core::types::VirtualsState;
use alloy_primitives::{Address, U256};

fn tax(a: U256, pct: u8) -> Option<U256> {
    a.checked_mul(U256::from(pct)).map(|v| v / U256::from(100))
}
pub(crate) fn principal(s: &VirtualsState, a: U256) -> Option<U256> {
    a.checked_sub(tax(a, s.buy_tax)?)?
        .checked_sub(tax(a, s.launch_tax)?)
}

pub fn charge(s: &VirtualsState, input: Address, swapped: &Swapped) -> Option<(U256, Address)> {
    if input == s.asset {
        Some((
            swapped
                .amount_in
                .checked_sub(principal(s, swapped.amount_in)?)?,
            s.asset,
        ))
    } else {
        let gross = s.reserves[1].checked_sub(
            s.invariant
                .checked_div(s.reserves[0].checked_add(swapped.amount_in)?)?,
        )?;
        Some((gross.checked_sub(swapped.amount_out)?, s.asset))
    }
}

fn forward(s: &VirtualsState, buy: bool, amount: U256) -> Option<Swapped> {
    if amount.is_zero()
        || s.invariant.is_zero()
        || (buy && u16::from(s.buy_tax) + u16::from(s.launch_tax) >= 100)
        || (!buy && s.sell_tax >= 100)
    {
        return None;
    }
    let (i, o) = if buy { (1, 0) } else { (0, 1) };
    let credited = if buy { principal(s, amount)? } else { amount };
    let remaining = s
        .invariant
        .checked_div(s.reserves[i].checked_add(credited)?)?;
    let gross = s.reserves[o].checked_sub(remaining)?;
    if gross > s.backing[o] || (buy && s.graduation.is_some_and(|threshold| remaining <= threshold))
    {
        return None;
    }
    let net = if buy {
        gross
    } else {
        gross.checked_sub(tax(gross, s.sell_tax)?)?
    };
    (!net.is_zero()).then_some(Swapped {
        amount_in: amount,
        amount_out: net,
        complete: true,
    })
}

fn inverse(s: &VirtualsState, buy: bool, want: U256) -> Option<U256> {
    if want.is_zero() || (!buy && s.sell_tax >= 100) {
        return None;
    }
    let (i, o) = if buy { (1, 0) } else { (0, 1) };
    let hundred = U256::from(100);
    let gross = if buy {
        want
    } else {
        (want - U256::ONE)
            .checked_mul(hundred)?
            .checked_div(U256::from(100 - s.sell_tax))?
            .checked_add(U256::ONE)?
    };
    let remaining = s.reserves[o].checked_sub(gross)?.checked_add(U256::ONE)?;
    let credited = (s.invariant / remaining)
        .checked_add(U256::ONE)?
        .saturating_sub(s.reserves[i])
        .max(U256::ONE);
    if !buy {
        return Some(credited);
    }
    let keep = 100u8.checked_sub(s.buy_tax)?.checked_sub(s.launch_tax)?;
    if keep == 0 {
        return None;
    }
    if s.launch_tax == 0 {
        return (credited - U256::ONE)
            .checked_mul(hundred)?
            .checked_div(U256::from(keep))?
            .checked_add(U256::ONE);
    }
    // Два отдельных floor могут уменьшить net при переходе между соседними суммами.
    // Каждые 100 единиц входа дают строго keep дополнительных единиц, остатки 0..99 проверяются точно.
    (0..100)
        .filter_map(|rem: u64| {
            let rem = U256::from(rem);
            let extra = principal(s, rem)?;
            credited
                .saturating_sub(extra)
                .div_ceil(U256::from(keep))
                .checked_mul(hundred)?
                .checked_add(rem)
        })
        .min()
}

pub fn quote(s: &VirtualsState, input: Address, exact_in: bool, amount: U256) -> Option<Swapped> {
    if input != s.agent && input != s.asset {
        return None;
    }
    let buy = input == s.asset;
    if exact_in {
        return forward(s, buy, amount);
    }
    let result = forward(s, buy, inverse(s, buy, amount)?)?;
    (result.amount_out >= amount).then_some(result)
}
