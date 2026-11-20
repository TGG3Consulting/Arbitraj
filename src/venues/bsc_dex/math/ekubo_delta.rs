//! Файл: venues/bsc_dex/math/ekubo_delta.rs
//! Что делает: суммы token0 / token1 между двумя ценами при ликвидности — точный порт ядра Ekubo (решение 447):
//! amount0Delta / amount1Delta и их «отсортированные» варианты по ценам 64.128, округление вверх / вниз как в контракте.
//! Откат контракта (Amount0DeltaOverflow, Amount1DeltaOverflow, переполнение fullMulDivUp) — None.
//! Кто вызывает: math/ekubo_swap.rs.
//! От чего зависит: alloy-primitives, math/full_math.rs, math/ekubo_sqrt.rs.
//! Настройки: нет.
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa: src/math/delta.sol L111-L123 (sortAndConvertToFixedSqrtRatios),
//! L126-L170 (amount0Delta, amount0DeltaSorted), L173-L219 (amount1Delta, amount1DeltaSorted). Solady: fullMulDivUp —
//! ceil, fullMulDivUnchecked — floor без проверки (результат меньше liquidityX128), fullMulDivN(x, y, 128) — floor(x·y / 2^128).

use alloy_primitives::U256;

use super::ekubo_sqrt::to_fixed;
use super::full_math::{mul_div, mul_div_up, mul_shift};

/// sortAndConvertToFixedSqrtRatios: две сжатые цены → (меньшая, большая) в 64.128. Ошибок нет.
pub fn sorted(a: U256, b: U256) -> (U256, U256) {
    let (x, y) = (to_fixed(a), to_fixed(b));
    if x > y { (y, x) } else { (x, y) }
}

/// amount0DeltaSorted (delta.sol L135-L170). Принимает: цены 64.128 (lower ≤ upper, ненулевые), ликвидность,
/// округление вверх. Возвращает: сумму token0. None — откат (результат больше 128 бит, переполнение).
pub fn amount0_sorted(lower: U256, upper: U256, liquidity: u128, round_up: bool) -> Option<u128> {
    if lower.is_zero() || upper.is_zero() {
        return None;
    }
    let lx = U256::from(liquidity) << 128usize;
    let diff = upper - lower;
    let r = if round_up {
        let r0 = mul_div_up(lx, diff, upper)?;
        let (mut q, rem) = r0.div_rem(lower);
        if !rem.is_zero() {
            q += U256::from(1u8);
        }
        q
    } else {
        mul_div(lx, diff, upper)? / lower
    };
    (r >> 128usize).is_zero().then(|| r.to::<u128>())
}

/// amount1DeltaSorted (delta.sol L181-L219). Принимает: цены 64.128 (lower ≤ upper), ликвидность, округление вверх.
/// Возвращает: сумму token1. None — откат (результат больше 128 бит).
pub fn amount1_sorted(lower: U256, upper: U256, liquidity: u128, round_up: bool) -> Option<u128> {
    let diff = upper - lower;
    let l = U256::from(liquidity);
    let mut r = mul_shift(diff, l, 128, false)?;
    if round_up {
        // mulmod(difference, liquidity, 2^128) ≠ 0 — младшие 128 бит произведения.
        let low = diff.wrapping_mul(l) & ((U256::from(1u8) << 128usize) - U256::from(1u8));
        if !low.is_zero() {
            r += U256::from(1u8);
        }
    }
    (r >> 128usize).is_zero().then(|| r.to::<u128>())
}

/// amount0Delta (delta.sol L126-L132): по двум сжатым ценам в любом порядке. None — откат.
pub fn amount0(a: U256, b: U256, liquidity: u128, round_up: bool) -> Option<u128> {
    let (lo, hi) = sorted(a, b);
    amount0_sorted(lo, hi, liquidity, round_up)
}

/// amount1Delta (delta.sol L173-L179): по двум сжатым ценам в любом порядке. None — откат.
pub fn amount1(a: U256, b: U256, liquidity: u128, round_up: bool) -> Option<u128> {
    let (lo, hi) = sorted(a, b);
    amount1_sorted(lo, hi, liquidity, round_up)
}

#[cfg(test)]
mod tests {
    use super::super::ekubo_sqrt::{MAX_SQRT_RATIO, MIN_SQRT_RATIO, to_sqrt_ratio};
    use super::*;

    fn one() -> U256 {
        U256::from((1u128 << 95) + (1u128 << 62))
    }

    fn ts(s: &str, add_one: bool) -> U256 {
        let mut v = U256::from_str_radix(s, 10).unwrap();
        if add_one {
            v += U256::from(1u8) << 128usize;
        }
        to_sqrt_ratio(v, false).unwrap()
    }

    /// Векторы официального теста ядра (evm-contracts@61d09daa test/math/delta.t.sol L24-L51, L75-L89).
    #[test]
    fn delta_official_vectors() {
        let (min, max) = (U256::from(MIN_SQRT_RATIO), U256::from(MAX_SQRT_RATIO));
        assert_eq!(
            amount0(one(), min, 1, false),
            Some(18_446_296_994_052_723_739)
        );
        assert_eq!(
            amount0(min, one(), 1, false),
            Some(18_446_296_994_052_723_739)
        );
        assert_eq!(amount0(min, min, u128::MAX, true), Some(0));
        assert_eq!(amount0(max, max, u128::MAX, false), Some(0));
        let a = ts("339942424496442021441932674757011200255", false);
        assert_eq!(amount0(a, one(), 1_000_000, false), Some(1000));
        assert_eq!(amount0(a, one(), 1_000_000, true), Some(1001));
        let b = ts("34028236692093846346337460743176821145", true);
        assert_eq!(
            amount0(b, one(), 1_000_000_000_000_000_000, true),
            Some(90_909_090_909_090_910)
        );
        let c = ts("340622989910849312776150758189957120", true);
        assert_eq!(amount0(c, one(), 1_000_000, false), Some(999));
        assert_eq!(
            amount1(one(), max, 1, false),
            Some(18_446_296_994_052_723_737)
        );
        assert_eq!(
            amount1(max, one(), 1, false),
            Some(18_446_296_994_052_723_737)
        );
        assert_eq!(amount1(min, min, u128::MAX, true), Some(0));
        let d = ts("309347606291762239512158734028880192232", false);
        assert_eq!(
            amount1(one(), d, 1_000_000_000_000_000_000, true),
            Some(90_909_090_909_090_910)
        );
        assert_eq!(
            amount1(one(), max, 0xffff_ffff_ffff_ffff, false),
            Some(340_274_119_756_928_397_675_478_831_269_759_003_622)
        );
    }
}
