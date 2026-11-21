//! Файл: venues/bsc_dex/math/ekubo_sqrt.rs
//! Что делает: сжатая цена Ekubo (SqrtRatio, 96 бит) ↔ фикс-точка 64.128, следующая цена по сумме token0 / token1 и
//! комиссия 0.64 (со входа и «до комиссии») — точный порт ядра Ekubo на U256 с теми же округлениями (решение 447).
//! Откат контракта — None; «переполнение» ядра, которое оно возвращает значением type(uint96).max, — то же значение.
//! Кто вызывает: math/ekubo_tick.rs, math/ekubo_swap.rs, math/ekubo_delta.rs.
//! От чего зависит: alloy-primitives, math/full_math.rs.
//! Настройки: нет.
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa716d5ee4f4814d901bf1b671db1d29db (ядро Arbitrum — broadcast
//! DeployAll.s.sol/42161 run-1767194711580.json, commit 61d09da; файлы ниже до HEAD e5574ab не менялись):
//! src/types/sqrtRatio.sol L13-L24 (константы), L59-L99 (toSqrtRatio), L102-L106 (toFixed);
//! src/math/sqrtRatio.sol L10-L64 (nextSqrtRatioFromAmount0), L67-L101 (nextSqrtRatioFromAmount1);
//! src/math/fee.sol L6-L10, L15-L25 (computeFee, amountBeforeFee). Solady FixedPointMathLib: fullMulDivUp = ceil(a·b/d) с
//! откатом при переполнении, rawDiv(a, 0) = 0, zeroFloorSub = max(a − b, 0).

use alloy_primitives::U256;

use super::full_math::mul_div_up;

/// MIN_SQRT_RATIO_RAW (types/sqrtRatio.sol L13).
pub const MIN_SQRT_RATIO: u128 = 4_611_797_791_050_542_631;
/// MAX_SQRT_RATIO_RAW (types/sqrtRatio.sol L15).
pub const MAX_SQRT_RATIO: u128 = 79_227_682_466_138_141_934_206_691_491;
/// type(uint96).max — «переполнение» nextSqrtRatioFromAmount0/1 (math/sqrtRatio.sol L33, L40, L48, L96).
pub const U96_MAX: u128 = (1u128 << 96) - 1;
/// BIT_MASK = 2^95 | 2^94 (types/sqrtRatio.sol L22).
const BIT_MASK: u128 = 0xc000_0000_0000_0000_0000_0000;

/// MAX_FIXED_VALUE_ROUND_UP = 2^192 − 2^98 (types/sqrtRatio.sol L55-L56).
fn max_fixed_round_up() -> U256 {
    (U256::from(1u8) << 192) - (U256::from(1u8) << 98)
}

/// toFixed: сжатая цена → 64.128. Принимает: сжатое значение (≤ 96 бит). Ошибок нет.
pub fn to_fixed(r: U256) -> U256 {
    let mask = U256::from(BIT_MASK);
    let shift = 2 + ((r & mask) >> 89usize).to::<usize>();
    (r & !mask) << shift
}

/// toSqrtRatio: 64.128 → сжатое значение с округлением вверх или вниз. None — ValueOverflowsSqrtRatioContainer.
pub fn to_sqrt_ratio(sr: U256, round_up: bool) -> Option<U256> {
    // (сдвиг, граница области, метка старших бит) — четыре области types/sqrtRatio.sol L59-L99.
    let regions: [(usize, usize, u128); 4] = [
        (2, 96, 0),
        (34, 128, 1u128 << 94),
        (66, 160, 1u128 << 95),
        (98, 192, 3u128 << 94),
    ];
    for (s, limit, tag) in regions {
        let add = if round_up {
            (U256::from(1u8) << s) - U256::from(1u8)
        } else {
            U256::ZERO
        };
        let v = sr.checked_add(add)?;
        if v < (U256::from(1u8) << limit) {
            return Some(U256::from(tag) | (v >> s));
        }
    }
    None
}

/// nextSqrtRatioFromAmount0 (sqrtRatio.sol L10-L64): цена после входа (amount > 0) или выхода (amount < 0) token0.
/// Принимает: сжатую цену (ненулевую), ликвидность (ненулевую), сумму. Возвращает: сжатую цену (U96_MAX —
/// переполнение по правилу ядра). None — откат (fullMulDivUp, toSqrtRatio).
pub fn next_from_amount0(sqrt: U256, liquidity: u128, amount: i128) -> Option<U256> {
    if amount == 0 {
        return Some(sqrt);
    }
    let r = to_fixed(sqrt);
    let lx = U256::from(liquidity) << 128usize;
    if amount < 0 {
        let abs = U256::from(amount.unsigned_abs());
        let lim = if r.is_zero() {
            U256::ZERO
        } else {
            U256::MAX / r
        };
        if abs > lim {
            return Some(U256::from(U96_MAX));
        }
        let product = r * abs;
        if product >= lx {
            return Some(U256::from(U96_MAX));
        }
        let fixed = mul_div_up(lx, r, lx - product)?;
        if fixed > max_fixed_round_up() {
            return Some(U256::from(U96_MAX));
        }
        to_sqrt_ratio(fixed, true)
    } else {
        // Ассемблер: div(liquidityX128, sqrtRatio) + amount; затем ceil(liquidityX128 / denominator).
        let q = if r.is_zero() { U256::ZERO } else { lx / r };
        let den = q + U256::from(amount as u128);
        let (mut raw, rem) = lx.div_rem(den);
        if !rem.is_zero() {
            raw += U256::from(1u8);
        }
        to_sqrt_ratio(raw, true)
    }
}

/// nextSqrtRatioFromAmount1 (sqrtRatio.sol L67-L101): цена после входа (amount > 0) или выхода (amount < 0) token1.
/// Принимает: сжатую цену, ликвидность (ненулевую), сумму. Возвращает: сжатую цену (U96_MAX — переполнение).
/// None — откат toSqrtRatio или нулевая ликвидность (ядро так не вызывает).
pub fn next_from_amount1(sqrt: U256, liquidity: u128, amount: i128) -> Option<U256> {
    if liquidity == 0 {
        return None;
    }
    let r = to_fixed(sqrt);
    let l = U256::from(liquidity);
    if amount < 0 {
        let num = U256::from(amount.unsigned_abs()) << 128usize;
        let (mut q, rem) = num.div_rem(l);
        if !rem.is_zero() {
            q += U256::from(1u8);
        }
        to_sqrt_ratio(r.saturating_sub(q), false)
    } else {
        let q = (U256::from(amount as u128) << 128usize) / l;
        let sum = r.checked_add(q)?;
        if sum > (U256::from(1u8) << 192) - U256::from(1u8) {
            return Some(U256::from(U96_MAX));
        }
        to_sqrt_ratio(sum, false)
    }
}

/// computeFee (fee.sol L6-L10): комиссия со суммы, вверх — (amount·fee + 2^64 − 1) >> 64. Ошибок нет.
pub fn compute_fee(amount: u128, fee: u64) -> u128 {
    let v = (U256::from(amount) * U256::from(fee) + U256::from(u64::MAX)) >> 64usize;
    v.to::<u128>()
}

/// amountBeforeFee (fee.sol L15-L25): сумма до комиссии, вверх — ceil((afterFee << 64) / (2^64 − fee)).
/// None — AmountBeforeFeeOverflow (результат больше 128 бит).
pub fn amount_before_fee(after: u128, fee: u64) -> Option<u128> {
    let v = U256::from(after) << 64usize;
    let d = (U256::from(1u8) << 64usize) - U256::from(fee);
    let (mut r, rem) = v.div_rem(d);
    if !rem.is_zero() {
        r += U256::from(1u8);
    }
    (r >> 128usize).is_zero().then(|| r.to::<u128>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(v: U256, up: bool) -> U256 {
        to_sqrt_ratio(v, up).unwrap()
    }

    fn one() -> U256 {
        U256::from((1u128 << 95) + (1u128 << 62))
    }

    fn big(s: &str) -> U256 {
        U256::from_str_radix(s, 10).unwrap()
    }

    /// ONE = 2^95 + 2^62 (types/sqrtRatio.sol L24) — это ровно 1.0 в 64.128, и обратно без потерь.
    #[test]
    fn one_round_trip() {
        assert_eq!(to_fixed(one()), U256::from(1u8) << 128usize);
        assert_eq!(
            to_sqrt_ratio(U256::from(1u8) << 128usize, false),
            Some(one())
        );
    }

    /// Векторы официального теста ядра (evm-contracts@61d09daa test/math/sqrtRatio.t.sol L15-L31, L85-L100).
    #[test]
    fn next_sqrt_ratio_official_vectors() {
        let l96 = 1u128 << 96;
        assert_eq!(
            next_from_amount0(one(), l96, 10000),
            Some(ts(big("340282366920938463463374564482095251457"), true))
        );
        assert_eq!(
            next_from_amount0(one(), l96, -10000),
            Some(ts(big("340282366920938463463374650381441171457"), true))
        );
        assert_eq!(
            next_from_amount0(one(), 1_000_000, 1000),
            Some(ts(big("339942424496442021441932674757011200256"), true))
        );
        assert_eq!(
            next_from_amount0(one(), 1, -100_000_000_000_000),
            Some(U256::from(U96_MAX))
        );
        assert_eq!(
            next_from_amount0(U256::from(MIN_SQRT_RATIO), 1, i128::MAX),
            Some(ts(U256::from(2u8), true))
        );
        let x = (U256::from(1u8) << 128usize) + big("3402823703237621667009962744418");
        assert_eq!(
            next_from_amount0(one(), 100_000_000_000, -1000),
            Some(ts(x, true))
        );
        let y = (U256::from(1u8) << 128usize) + big("340282366920938463463374607431768211");
        assert_eq!(
            next_from_amount1(one(), 1_000_000, 1000),
            Some(ts(y, false))
        );
        assert_eq!(
            next_from_amount1(one(), 1_000_000, -1000),
            Some(ts(big("339942084554017524999911232824336443244"), false))
        );
        assert_eq!(next_from_amount1(one(), 1, -1_000_000), Some(U256::ZERO));
        assert_eq!(
            next_from_amount1(U256::from(U96_MAX - 1), 1, i128::MAX),
            Some(U256::from(U96_MAX))
        );
    }

    /// Векторы официального теста ядра (test/math/fee.t.sol L8-L14, L35-L39).
    #[test]
    fn fee_official_vectors() {
        assert_eq!(compute_fee(100, u64::MAX), 100);
        assert_eq!(compute_fee(100, 1 << 63), 50);
        assert_eq!(compute_fee(100, 1 << 62), 25);
        assert_eq!(compute_fee(100, 1 << 61), 13);
        assert_eq!(
            compute_fee(u128::MAX, u64::MAX),
            340_282_366_920_938_463_444_927_863_358_058_659_840
        );
        assert_eq!(amount_before_fee(1, 1 << 63), Some(2));
        assert_eq!(amount_before_fee(2, 1 << 62), Some(3));
        assert_eq!(amount_before_fee(u128::MAX, 0), Some(u128::MAX));
    }
}
