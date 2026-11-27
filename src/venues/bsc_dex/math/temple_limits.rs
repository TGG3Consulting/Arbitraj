//! Exact input limits for a physical Temple payout, including integer price islands and the pair's balance-based K.
use super::*;

struct PairLimit {
    ri: U256,
    ro: U256,
    bi: U256,
    bo: U256,
    k: U256,
}
impl PairLimit {
    fn adjusted(&self, x: U256) -> U256 {
        let balance = self.bi + x;
        balance * U256::from(1000) - balance.saturating_sub(self.ri) * U256::from(3)
    }
    fn out_base(&self) -> U256 {
        self.bo * U256::from(1000) - self.bo.saturating_sub(self.ro) * U256::from(3)
    }
    /// Multiply the upper bound obtained from floor(q) > q-1 by the positive
    /// denominator. The pair's exact floor can increase the available balance by at most (den-1)/den.
    /// On either side of balanceIn=reserveIn this is quadratic; store positive and negative parts separately.
    fn upper(&self, x: U256) -> (U512, U512) {
        let f = U512::from(995) * U512::from(x);
        let den = U512::from(1000) * U512::from(self.ri) + f;
        let a = U512::from(self.adjusted(x));
        (
            a * U512::from(self.out_base() + U256::from(1000)) * den,
            a * (U512::from(1000) * (U512::from(self.ro) * f + U512::ONE))
                + U512::from(self.k) * den,
        )
    }
    fn possible(&self, lo: U256, hi: U256) -> bool {
        // The router takes 0.5%, the pair tests 0.3%. With no deficits the K test is always satisfied.
        if self.bi >= self.ri && self.bo >= self.ro {
            return true;
        }
        let q = |x| v2::amount_out(x, self.ri, self.ro, (U256::from(995), U256::from(1000)));
        let Some(first) = q(lo) else {
            return false;
        };
        let Some(last) = q(hi) else {
            return false;
        };
        let max = (U256::ONE << 112usize) - U256::ONE;
        if last.is_zero() || first > self.bo || self.bo.saturating_sub(last) > max {
            return false;
        }
        let Some(out) = self.out_base().checked_sub(first * U256::from(1000)) else {
            return false;
        };
        if self.adjusted(hi) * out < self.k {
            return false;
        }
        let kink = self.ri.saturating_sub(self.bi);
        if lo < kink && kink < hi {
            return self.possible(lo, kink) || self.possible(kink, hi);
        }
        let diff = |x| self.upper(x);
        let good = |(p, n): (U512, U512)| p >= n;
        if good(diff(lo)) || good(diff(hi)) {
            return true;
        }
        if hi <= lo + U256::ONE || self.out_base() + U256::from(1000) >= self.ro * U256::from(1000)
        {
            return false;
        }
        // Concave quadratic. Its integer maximum occurs where the first difference changes sign.
        let rising = |x| {
            let (p, n) = diff(x);
            let (pp, nn) = diff(x + U256::ONE);
            pp + n > p + nn
        };
        if !rising(lo) || rising(hi - U256::ONE) {
            return false;
        }
        let (mut l, mut h) = (lo, hi);
        while l < h {
            let m = l + (h - l) / U256::from(2);
            if rising(m) {
                l = m + U256::ONE;
            } else {
                h = m;
            }
        }
        good(diff(l))
    }
}

pub(super) fn find(
    s: &TempleState,
    buy: bool,
    defense: bool,
    mut lo: U256,
    mut hi: U256,
    last: bool,
) -> Option<U256> {
    if s.reserves.contains(&U256::ZERO) || (!defense && !s.amm_authorized) {
        return None;
    }
    let i = usize::from(buy);
    let (ri, ro) = (s.reserves[i], s.reserves[1 - i]);
    let fee = U256::from(995);
    hi = hi
        .min(U256::MAX / ro / fee)
        .min((U256::MAX - ri.checked_mul(U256::from(1000))?) / fee);
    let ratio = if buy {
        None
    } else {
        let [n, d] = s.ratio?;
        if d.is_zero() {
            return None;
        }
        if !n.is_zero() {
            hi = hi.min(U256::MAX / n);
        }
        Some((n, d))
    };
    if defense {
        let (n, d) = ratio?;
        if n.is_zero() || s.defense_balance.is_zero() {
            return None;
        }
        lo = lo.max(crossover(s, n, d)?);
        if let Some(x) = s
            .defense_balance
            .checked_add(U256::ONE)
            .and_then(|v| mul_div_up(v, d, n))
        {
            hi = hi.min(x.saturating_sub(U256::ONE));
        }
    }
    let pair = if defense {
        None
    } else {
        let max = (U256::ONE << 112usize) - U256::ONE;
        let (bi, bo) = (s.balances[i], s.balances[1 - i]);
        hi = hi.min(max.checked_sub(bi)?);
        // q<ro. A balance larger than max112+ro cannot be saved by any swap.
        if bo > max + ro {
            return None;
        }
        // The output balance must fit in uint112 even if current balance exceeds the recorded reserve.
        lo = lo.max(amm_input(s, buy, bo.saturating_sub(max).max(U256::ONE))?);
        Some(PairLimit {
            ri,
            ro,
            bi,
            bo,
            k: ri.checked_mul(ro)?.checked_mul(U256::from(1_000_000))?,
        })
    };
    let mut ranges = vec![(lo.max(U256::ONE), hi)];
    while let Some((l, h)) = ranges.pop() {
        if l > h {
            continue;
        }
        let x = if last { h } else { l };
        let (branch, a, iv) = amounts(s, buy, x)?;
        if branch == defense
            && if defense {
                !iv.is_zero() && iv <= s.defense_balance
            } else {
                !a.is_zero() && amm_after(s, i, x, a).is_some()
            }
        {
            return Some(x);
        }
        if l == h {
            continue;
        }
        if let Some((n, d)) = ratio
            && !rounding::possible(s, l, h, n, d, defense)
        {
            continue;
        }
        if let Some(p) = &pair
            && !p.possible(l, h)
        {
            continue;
        }
        let m = l + (h - l) / U256::from(2);
        let (left, right) = ((l, m), (m + U256::ONE, h));
        if last {
            ranges.push(left);
            ranges.push(right);
        } else {
            ranges.push(right);
            ranges.push(left);
        }
    }
    None
}
