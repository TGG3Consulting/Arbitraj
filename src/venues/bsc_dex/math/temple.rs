//! TempleStableAMMRouter compares two independently floored quotes. Equality stays on the AMM.
use super::{full_math::mul_div_up, swap_cl::Swapped, v2};
use crate::core::types::TempleState;
use alloy_primitives::{Address, U256, U512};

#[path = "temple_limits.rs"]
mod limits;
#[path = "temple_rounding.rs"]
mod rounding;
#[cfg(test)]
#[path = "temple_tests.rs"]
mod tests;

fn amm(s: &TempleState, buy: bool, x: U256) -> Option<U256> {
    let i = usize::from(buy);
    v2::amount_out(
        x,
        s.reserves[i],
        s.reserves[1 - i],
        (U256::from(995), U256::from(1000)),
    )
}

fn amm_input(s: &TempleState, buy: bool, want: U256) -> Option<U256> {
    let i = usize::from(buy);
    let left = s.reserves[1 - i].checked_sub(want)?;
    mul_div_up(
        want,
        s.reserves[i].checked_mul(U256::from(1000))?,
        left.checked_mul(U256::from(995))?,
    )
}

fn amounts(s: &TempleState, buy: bool, x: U256) -> Option<(bool, U256, U256)> {
    let a = amm(s, buy, x)?;
    if buy {
        return Some((false, a, U256::ZERO));
    }
    let [n, d] = s.ratio?;
    let iv = x.checked_mul(n)?.checked_div(d)?;
    Some((iv > a, a, iv))
}

pub fn is_defense(s: &TempleState, input: Address, size: U256) -> Option<bool> {
    if input != s.temple && input != s.stable {
        return None;
    }
    amounts(s, input == s.stable, size).map(|t| t.0)
}

/// The router charges 0.5%. The physical pair independently tests its 0.3% invariant
/// and saves *balances* as the new reserves; an unsolicited transfer cannot increase the quote.
fn amm_after(s: &TempleState, i: usize, x: U256, y: U256) -> Option<[U256; 2]> {
    let mut b = s.balances;
    b[i] = b[i].checked_add(x)?;
    b[1 - i] = b[1 - i].checked_sub(y)?;
    if !s.amm_authorized || b.iter().any(|v| v.bit_len() > 112) {
        return None;
    }
    let mut adjusted = [U256::ZERO; 2];
    for k in 0..2 {
        let prior = if k == i {
            s.reserves[k]
        } else {
            s.reserves[k].checked_sub(y)?
        };
        let incoming = b[k].saturating_sub(prior);
        adjusted[k] = b[k] * U256::from(1000) - incoming * U256::from(3);
    }
    let invariant = s.reserves[0]
        .checked_mul(s.reserves[1])?
        .checked_mul(U256::from(1_000_000))?;
    (adjusted[0] * adjusted[1] >= invariant).then_some(b)
}

pub fn paid(
    s: &TempleState,
    input: Address,
    output: Address,
    x: U256,
) -> Option<(Swapped, TempleState)> {
    if input == output || x.is_zero() {
        return None;
    }
    let buy = input == s.stable;
    if !buy && input != s.temple {
        return None;
    }
    let (defense, a, iv) = amounts(s, buy, x)?;
    let (asset, y) = if defense {
        (s.defend, iv)
    } else if buy {
        (s.temple, a)
    } else {
        (s.stable, a)
    };
    if output != asset || y.is_zero() || asset.is_zero() {
        return None;
    }
    let mut next = s.clone();
    if defense {
        next.defense_balance = s.defense_balance.checked_sub(y)?;
    } else {
        let balances = amm_after(s, usize::from(buy), x, y)?;
        next.reserves = balances;
        next.balances = balances;
    }
    Some((
        Swapped {
            amount_in: x,
            amount_out: y,
            complete: true,
        },
        next,
    ))
}

// First integer at which the unrounded IV could be strictly above the unrounded AMM.
fn crossover(s: &TempleState, n: U256, d: U256) -> Option<U256> {
    let fee = U512::from(995);
    let denominator = fee * U512::from(n);
    if denominator.is_zero() {
        return None;
    }
    let left = fee * U512::from(s.reserves[1]) * U512::from(d);
    let right = U512::from(1000) * U512::from(s.reserves[0]) * U512::from(n);
    let x = left.saturating_sub(right) / denominator + U512::ONE;
    (x <= U512::from(U256::MAX)).then(|| U256::from_limbs_slice(&x.as_limbs()[..4]))
}

fn inverse(s: &TempleState, buy: bool, defense: bool, want: U256) -> Option<U256> {
    let lo = if defense {
        let [n, d] = s.ratio?;
        if want > s.defense_balance {
            return None;
        }
        mul_div_up(want, d, n)?.max(crossover(s, n, d)?)
    } else {
        amm_input(s, buy, want)?
    };
    limits::find(s, buy, defense, lo, U256::MAX, false)
}

pub fn quote(
    s: &TempleState,
    input: Address,
    output: Address,
    exact_in: bool,
    want: U256,
) -> Option<Swapped> {
    if exact_in {
        return paid(s, input, output, want).map(|p| p.0);
    }
    if want.is_zero() || input == output {
        return None;
    }
    let buy = input == s.stable && output == s.temple;
    if !buy && input != s.temple {
        return None;
    }
    [false, true]
        .into_iter()
        .filter(|&defense| {
            if buy {
                !defense
            } else if defense {
                output == s.defend
            } else {
                output == s.stable
            }
        })
        .filter_map(|defense| inverse(s, buy, defense, want))
        .filter_map(|x| paid(s, input, output, x).map(|p| p.0))
        .filter(|fill| fill.amount_out >= want)
        .min_by_key(|fill| fill.amount_in)
}

/// AMM depth uses the project's fee-free band; the router still selects the asset using its fee.
/// Treasury depth is limited by the actual ERC20 balance and its fixed exchange ratio.
pub fn band(
    s: &TempleState,
    input: Address,
    output: Address,
    grow: U256,
    scale: U256,
    cash_in: bool,
) -> U256 {
    let mut best = U256::ZERO;
    let buy = input == s.stable && output == s.temple;
    if s.amm_authorized && (buy || input == s.temple && output == s.stable) {
        let i = usize::from(buy);
        let cap = super::full_math::mul_div(s.reserves[i], grow, scale).unwrap_or_default();
        if let Some(x) = limits::find(s, buy, false, U256::ONE, cap, true)
            && let Some((fill, _)) = paid(s, input, output, x)
        {
            let cap = x;
            best = if cash_in {
                fill.amount_in
            } else {
                v2::amount_out(
                    cap,
                    s.reserves[i],
                    s.reserves[1 - i],
                    (U256::ONE, U256::ONE),
                )
                .unwrap_or_default()
            };
        }
    }
    if input == s.temple
        && output == s.defend
        && let Some(x) = limits::find(s, false, true, U256::ONE, U256::MAX, true)
        && let Some((fill, _)) = paid(s, input, output, x)
    {
        best = best.max(if cash_in {
            fill.amount_in
        } else {
            fill.amount_out
        });
    }
    best
}
