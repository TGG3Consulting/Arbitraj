//! Подбор входа для покупки в Curve-пуле Spectra. Проверяется выплата той же функции
//! точного входа, которая оценивает продажу. Вход и выход измеряются в атомах IBT/PT.

use alloy_primitives::U256;

use super::super::math::{full_math, swap_cl::Swapped, swap_solidly};
use super::super::pools::registry::Entry;
use crate::core::types::PoolState;

#[path = "spectra_legacy.rs"]
mod legacy;
use legacy::crypto_input;

pub(super) fn target(
    e: &Entry,
    zero_for_one: bool,
    wanted: U256,
    payout: impl Fn(U256) -> Option<U256>,
) -> Option<Swapped> {
    let (a, b, balances) = match &e.pool.state {
        PoolState::StableSwap(d) if !d.halted && d.ng.is_some() => (d.i0, d.i1, &d.balances),
        PoolState::Crypto(d) if !d.halted && !d.d.is_zero() => (d.i0, d.i1, &d.balances),
        _ => return None,
    };
    let (i, j) = if zero_for_one { (a, b) } else { (b, a) };
    let reserve_out = *balances.get(usize::from(j))?;
    if wanted.is_zero() || wanted >= reserve_out {
        return None;
    }
    // Отношение запасов задаёт только начальный размер поиска. Каждый последующий шаг — расчёт на том же состоянии.
    let reserve_in = *balances.get(usize::from(i))?;
    if reserve_in.is_zero() {
        return None;
    }
    let seed = full_math::mul_div_up(wanted, reserve_in, reserve_out)?;
    let cap = U256::MAX - reserve_in;
    let (paid, received) = match &e.pool.state {
        PoolState::Crypto(data) => {
            let price = |dx| {
                super::super::math::crypto_swap::exchange_out(
                    data,
                    usize::from(i),
                    usize::from(j),
                    dx,
                )
            };
            let hint = super::spectra_points::previous(e, zero_for_one, wanted);
            let result = bounded_input(hint, cap, wanted, price, |max| {
                crypto_input(seed, max, wanted, data, usize::from(i), price)
            })?;
            super::spectra_points::remember(e, zero_for_one, result.0, result.1);
            result
        }
        _ => first_input(seed, cap, wanted, payout)?,
    };
    (received <= reserve_out).then_some(Swapped {
        amount_in: paid,
        amount_out: received,
        complete: true,
    })
}

/// Ранее найденный вход даёт предел списания только после подтверждения выплаты на текущем состоянии.
fn bounded_input(
    hint: Option<U256>,
    cap: U256,
    wanted: U256,
    payout: impl Fn(U256) -> Option<U256>,
    solve: impl FnOnce(U256) -> Option<(U256, U256)>,
) -> Option<(U256, U256)> {
    let known = hint
        .filter(|x| !x.is_zero() && *x <= cap)
        .and_then(|x| payout(x).filter(|y| *y >= wanted).map(|y| (x, y)));
    let upper = known.map_or(cap, |v| v.0);
    solve(upper)
        .filter(|(x, y)| *x <= upper && *y >= wanted)
        .or(known)
}

/// Ищет границу только по прямым котировкам. Слишком маленький и чрезмерный вход могут оба дать откат.
fn first_input(
    seed: U256,
    cap: U256,
    wanted: U256,
    payout: impl Fn(U256) -> Option<U256>,
) -> Option<(U256, U256)> {
    if cap.is_zero() || wanted.is_zero() {
        return None;
    }
    let mut high = seed.clamp(U256::ONE, cap);
    let mut low = high;
    let mut sample = payout(high).filter(|x| !x.is_zero()).map(|out| (high, out));
    while sample.is_none() && (low > U256::ONE || high < cap) {
        if low > U256::ONE {
            low >>= 1;
            sample = payout(low).filter(|x| !x.is_zero()).map(|out| (low, out));
        }
        if sample.is_none() && high < cap {
            high = high.saturating_mul(U256::from(2)).min(cap);
            sample = payout(high).filter(|x| !x.is_zero()).map(|out| (high, out));
        }
    }
    let (mut paid, mut out) = sample?;
    // Секущая даёт предполагаемый вход, но цену подтверждает только прямой расчёт.
    // Из каждой пары сохраняем самую дешёвую уже проверенную выплату.
    let mut known = (out >= wanted).then_some((paid, out));
    let mut proposal = full_math::mul_div_up(wanted, paid, out);
    for _ in 0..5 {
        let Some(x) = proposal
            .map(|x| x.clamp(U256::ONE, cap))
            .filter(|x| *x != paid)
        else {
            break;
        };
        let Some(y) = payout(x).filter(|v| !v.is_zero()) else {
            break;
        };
        if y >= wanted && known.is_none_or(|(old, _)| x < old) {
            known = Some((x, y));
        }
        if y == wanted {
            break;
        }
        if (x > paid) != (y > out) || y == out {
            break;
        }
        let offset = full_math::mul_div_up(wanted.abs_diff(y), x.abs_diff(paid), y.abs_diff(out));
        proposal = offset.map(|v| {
            if y < wanted {
                x.saturating_add(v)
            } else {
                x.saturating_sub(v)
            }
        });
        (paid, out) = (x, y);
    }
    if let Some((x, y)) = known {
        (paid, out) = (x, y);
    }
    if out >= wanted && (paid == U256::ONE || payout(paid - U256::ONE).is_none_or(|v| v < wanted)) {
        return Some((paid, out));
    }
    // Если удвоение перескочило последний допустимый вход, сужаем диапазон к известной выполнимой точке.
    let mut end = cap;
    while out < wanted {
        if paid >= end {
            return None;
        }
        let mut next = paid.saturating_mul(U256::from(2)).min(end);
        if let Some(y) = payout(next) {
            (paid, out) = (next, y);
        } else {
            end = next - U256::ONE;
            if paid >= end {
                return None;
            }
            next = paid + (end - paid).div_ceil(U256::from(2));
            match payout(next) {
                Some(y) => (paid, out) = (next, y),
                None => end = next - U256::ONE,
            }
        }
    }
    // Близкую границу ищем от оплаченного входа. Если цена далека, обычное деление пополам остаётся ограниченным.
    let mut span = U256::ONE;
    let top = paid;
    for _ in 0..8 {
        let below = top.saturating_sub(span);
        match (!below.is_zero()).then(|| payout(below)).flatten() {
            Some(y) if y >= wanted => {
                (paid, out) = (below, y);
                span = span.saturating_mul(U256::from(2));
            }
            _ => {
                let mut low = below;
                while paid - low > U256::ONE {
                    let mid = low + (paid - low) / U256::from(2);
                    if let Some(y) = payout(mid).filter(|y| *y >= wanted) {
                        (paid, out) = (mid, y);
                    } else {
                        low = mid;
                    }
                }
                return Some((paid, out));
            }
        }
    }
    let minimum = swap_solidly::input_for(paid, wanted, &payout)?;
    let received = payout(minimum)?;
    (received >= wanted
        && (minimum == U256::ONE || payout(minimum - U256::ONE).is_none_or(|v| v < wanted)))
    .then_some((minimum, received))
}

#[cfg(test)]
#[path = "spectra_tests.rs"]
mod tests;
