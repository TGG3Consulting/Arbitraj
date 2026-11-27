//! Exact first eligible input at a rounded Temple treasury/AMM crossing.
//! Above the rational crossing, real IV minus real AMM is increasing. Integer
//! eligibility depends on the remainder n*x mod d. Whole intervals can therefore
//! be excluded using their remainder extrema without walking every raw token unit.
use super::*;

fn narrow(v: U512) -> U256 {
    U256::from_limbs_slice(&v.as_limbs()[..4])
}
fn modular(a: U256, b: U256, offset: U256, m: U256) -> U256 {
    narrow((U512::from(a) * U512::from(offset) + U512::from(b)) % U512::from(m))
}

/// Minimum (a*i + b) % m on 0 <= i < len. After reversing a steep progression,
/// each wrap contributes its first residue to a progression of modulus <= m/2.
fn min_residue(mut a: U256, mut b: U256, mut m: U256, mut len: U256) -> U256 {
    let mut best = b;
    loop {
        if a > m / U256::from(2) {
            b = modular(a, b, len - U256::ONE, m);
            a = m - a;
        }
        best = best.min(b);
        if best.is_zero() || a.is_zero() {
            return best;
        }
        let k =
            narrow((U512::from(a) * U512::from(len - U256::ONE) + U512::from(b)) / U512::from(m));
        if k.is_zero() {
            return best;
        }
        let step = (a - m % a) % a;
        b = modular(U256::ONE, b % a, step, a);
        m = a;
        a = step;
        len = k;
    }
}

/// Compare an arbitrary IV remainder with d * (unrounded IV - unrounded AMM).
fn above(s: &TempleState, x: U256, n: U256, d: U256, rem: U256) -> bool {
    let (iv, ir) = (x * n).div_rem(d);
    let adjusted = x * U256::from(995);
    let den = adjusted + s.reserves[0] * U256::from(1000);
    let (a, ar) = (adjusted * s.reserves[1]).div_rem(den);
    if iv < a {
        return false;
    }
    let diff = iv - a;
    if diff > U256::ONE || (diff == U256::ONE && ir >= rem) {
        return true;
    }
    let t = if diff.is_zero() {
        let Some(v) = ir.checked_sub(rem) else {
            return false;
        };
        v
    } else {
        d - (rem - ir)
    };
    U512::from(t) * U512::from(den) > U512::from(ar) * U512::from(d)
}

pub(super) fn first(s: &TempleState, low: U256, n: U256, d: U256, defense: bool) -> Option<U256> {
    if n.is_zero() || d.is_zero() || s.reserves.contains(&U256::ZERO) {
        return None;
    }
    let fee = U256::from(995);
    let mut high = (U256::MAX / n)
        .min(U256::MAX / s.reserves[1] / fee)
        .min((U256::MAX - s.reserves[0].checked_mul(U256::from(1000))?) / fee);
    if defense {
        if let Some(v) = s
            .defense_balance
            .checked_add(U256::ONE)
            .and_then(|v| mul_div_up(v, d, n))
        {
            high = high.min(v.saturating_sub(U256::ONE));
        }
    } else {
        high = high.min(((U256::ONE << 112usize) - U256::ONE).saturating_sub(s.balances[0]));
    }
    let mut intervals = vec![(low, high)];
    while let Some((lo, hi)) = intervals.pop() {
        if lo > hi {
            continue;
        }
        let a = n % d;
        let b = modular(a, U256::ZERO, lo, d);
        if above(s, lo, n, d, b) == defense {
            return Some(lo);
        }
        if lo == hi {
            continue;
        }
        let len = hi - lo + U256::ONE;
        let possible = if defense {
            above(s, hi, n, d, min_residue(a, b, d, len))
        } else {
            let maximum = d - U256::ONE - min_residue((d - a) % d, d - U256::ONE - b, d, len);
            !above(s, lo, n, d, maximum)
        };
        if possible {
            let mid = lo + (hi - lo) / U256::from(2);
            intervals.push((mid + U256::ONE, hi));
            intervals.push((lo, mid));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn euclidean_extrema_match_enumeration_in_both_orientations() {
        for m in 1u64..=15 {
            for a in 0..m {
                for b in 0..m {
                    for len in [1, 2, 7, 17, 40] {
                        let minimum = (0..len).map(|k| (a * k + b) % m).min().unwrap();
                        assert_eq!(
                            min_residue(
                                U256::from(a),
                                U256::from(b),
                                U256::from(m),
                                U256::from(len)
                            ),
                            U256::from(minimum)
                        );
                    }
                }
            }
        }
    }
}
