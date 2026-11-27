//! Наименьший целый вход для заданной выплаты. Вызывает MooniswapState::swap.
//! В Liquidity Protocol выплата после двух округлений может упасть при увеличении
//! входа. Используем границы по первому результату деления и точную проверку forward.
//! https://github.com/1inch/liquidity-protocol/blob/48d3db12/contracts/Mooniswap.sol
use super::{MooniswapState, Swapped, U256, scale};

#[path = "mooniswap_lattice.rs"]
mod lattice;

type Wide = alloy_primitives::Uint<768, 12>;

/// Огибающая по целому gross. При g=floor(t*y/(x+t)) доля t/(x+t) >= g/y,
/// поэтому g*(y*D-slippage*g) >= target*y*D необходимо для любой выплаты.
fn interval(y: U256, want: U256, slippage: U256) -> Option<(U256, U256)> {
    let last = y.checked_sub(U256::ONE)?;
    if want > last || slippage > scale() {
        return None;
    }
    if slippage.is_zero() {
        return Some((want, last));
    }
    let yd = Wide::from(y) * Wide::from(scale());
    let threshold = Wide::from(want) * yd;
    let value = |g: U256| Wide::from(g) * (yd - Wide::from(slippage) * Wide::from(g));
    let mut peak = if slippage <= scale() / U256::from(2) {
        last
    } else {
        super::super::full_math::mul_div(y, scale(), slippage * U256::from(2))?.min(last)
    };
    if peak < last && value(peak + U256::ONE) > value(peak) {
        peak += U256::ONE
    }
    if value(peak) < threshold {
        return None;
    }
    let (mut lo, mut hi) = (U256::ZERO, peak);
    while lo < hi {
        let m = lo + ((hi - lo) >> 1);
        if value(m) >= threshold {
            hi = m
        } else {
            lo = m + U256::ONE
        }
    }
    let start = lo;
    (lo, hi) = (peak, last);
    while lo < hi {
        let m = hi - ((hi - lo) >> 1);
        if value(m) >= threshold {
            lo = m
        } else {
            hi = m - U256::ONE
        }
    }
    Some((start, hi))
}

pub(super) fn first(
    state: &MooniswapState,
    x: U256,
    y: U256,
    amount: U256,
    mut inspected: impl FnMut(),
) -> Option<Swapped> {
    let (mut gross, last) = interval(y, amount, state.slippage_fee)?;
    while gross <= last {
        inspected();
        let taxed = super::super::full_math::mul_div_up(gross, x, y - gross)?;
        let dx = super::super::full_math::mul_div(
            taxed.checked_sub(U256::ONE)?,
            scale(),
            scale() - state.fee,
        )?
        .checked_add(U256::ONE)?;
        let (out, num, den) = state.forward(x, y, dx)?;
        if out >= amount {
            return Some(Swapped {
                amount_in: dx,
                amount_out: out,
                complete: true,
            });
        }
        if num.is_zero() {
            return None;
        }
        let next = super::super::full_math::mul_div_up(amount, den, num)?;
        if next <= gross {
            return None;
        }
        gross = if next < last {
            // По истечении срока вызывающий уже снял запрос котировки.
            if crate::venues::bsc_dex::quote::abandon::is_set() {
                return None;
            }
            lattice::skip(x, y, amount, state.slippage_fee, next, last)
        } else {
            next
        };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Address;

    #[test]
    fn rounded_peak_with_billion_scale_and_fractional_slopes() {
        let n = U256::from(1_000_000_000u64);
        let want = n * n - U256::ONE;
        let y = U256::from(4) * n * n;
        let mut evidence = Vec::new();
        for divisor in [1, 2, 3, 17] {
            let x = (n * n + n / U256::from(8)) / U256::from(divisor);
            let tokens = [Address::ZERO, Address::repeat_byte(1)];
            let state = MooniswapState {
                physical: tokens,
                tokens,
                balances: [x, y],
                addition: [(U256::ZERO, 0); 2],
                removal: [(U256::ZERO, 0); 2],
                timestamp: 1000,
                decay: 80,
                fee: U256::ZERO,
                slippage_fee: scale(),
                successor: true,
            };
            let mut probes = 0;
            let fill = first(&state, x, y, want, || {
                probes += 1;
                assert!(probes < 256, "slope 1/{divisor}");
            })
            .unwrap();
            assert_eq!(
                state.forward(x, y, fill.amount_in).unwrap().0,
                fill.amount_out
            );
            assert!(fill.amount_out >= want);
            assert!(state.forward(x, y, fill.amount_in - U256::ONE).unwrap().0 < want);
            evidence.push(serde_json::json!({
                "divisor": divisor, "x": x.to_string(), "y": y.to_string(), "target": want.to_string(),
                "first_input": fill.amount_in.to_string(), "output": fill.amount_out.to_string(),
                "forward_evaluations": probes
            }));
        }
        if let Some(path) = std::env::var_os("ARB_MOONISWAP_INVERSE_EXPORT") {
            std::fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
        }
    }
}
