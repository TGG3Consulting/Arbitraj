//! Файл: venues/bsc_dex/math/ekubo_twamm.rs
//! Что делает: математика TWAMM Ekubo (решение 454) — точный порт: сумма продажи из скорости за время, скорость ± дельта
//! (с откатом за 112 бит), sqrt отношения скоростей, c = (√скорость − √цена)/(√скорость + √цена) и цена пула после
//! исполнения виртуальных ордеров за время (computeNextSqrtRatio). Нужна котировке пула с расширением TWAMM: перед нашим
//! обменом ядро зовёт TWAMM.beforeSwap, и тот исполняет виртуальные ордера до времени блока (math/ekubo_virtual.rs).
//! Кто вызывает: math/ekubo_virtual.rs.
//! От чего зависит: alloy-primitives, math/ekubo_exp2.rs, math/ekubo_sqrt.rs (to_fixed, to_sqrt_ratio, compute_fee),
//! math/full_math.rs (mul_div).
//! Настройки: нет.
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa src/math/twamm.sol: L28-L38 (addSaleRateDelta), L42-L46
//! (computeAmountFromSaleRate), L56-L64 (computeC), L68-L83 (computeSqrtSaleRatio), L89-L142 (computeNextSqrtRatio).
//! solady FixedPointMathLib (lib/solady): rawDiv — деление, на 0 — 0; sqrt — вниз; fullMulDiv — вниз, на 0 и
//! переполнение — откат; dist — |a − b|; abs — модуль.

use alloy_primitives::{I256, U256};

use super::ekubo_exp2::exp2;
use super::ekubo_sqrt::{compute_fee, to_fixed, to_sqrt_ratio};
use super::full_math::mul_div;

/// 2^112 − 1: предел скорости продажи (uint112).
fn u112_max() -> U256 {
    (U256::from(1u8) << 112usize) - U256::from(1u8)
}

/// (saleRate · duration + (roundUp ? 2^32 − 1 : 0)) >> 32. Ошибок нет.
pub fn amount_from_sale_rate(rate: u128, duration: u64, round_up: bool) -> U256 {
    let add = if round_up { u64::from(u32::MAX) } else { 0 };
    (U256::from(rate) * U256::from(duration) + U256::from(add)) >> 32usize
}

/// Скорость + дельта. None — итог вне uint112 (SaleRateDeltaOverflow — откат). Ошибок нет.
pub fn add_sale_rate_delta(rate: u128, delta: i128) -> Option<u128> {
    let r = i128::try_from(rate).ok()?.checked_add(delta)?;
    (r >= 0 && U256::from(r as u128) <= u112_max()).then_some(r as u128)
}

/// computeC: со знаком 64.128. None — сумма 0 (fullMulDiv на 0). Ошибок нет.
pub fn compute_c(sqrt_ratio: U256, sqrt_sale_ratio: U256) -> Option<I256> {
    let dist = if sqrt_ratio > sqrt_sale_ratio {
        sqrt_ratio - sqrt_sale_ratio
    } else {
        sqrt_sale_ratio - sqrt_ratio
    };
    let unsigned = mul_div(
        dist,
        U256::from(1u8) << 128usize,
        sqrt_ratio + sqrt_sale_ratio,
    )?;
    let v = I256::from_raw(unsigned);
    if sqrt_sale_ratio > sqrt_ratio {
        Some(v)
    } else {
        Some(-v)
    }
}

/// computeSqrtSaleRatio: √(r1/r0) в 64.128 (оба > 0, ≤ uint112). Ошибок нет.
pub fn sqrt_sale_ratio(rate0: u128, rate1: u128) -> U256 {
    let ratio = (U256::from(rate1) << 128usize)
        .checked_div(U256::from(rate0))
        .unwrap_or_default();
    if ratio <= U256::from(u128::MAX) {
        (ratio << 128usize).root(2)
    } else if ratio < U256::from(1u8) << 192usize {
        (ratio << 64usize).root(2) << 32usize
    } else {
        (ratio << 16usize).root(2) << 56usize
    }
}

/// computeNextSqrtRatio: сжатая цена после виртуальных ордеров за время. Принимает: сжатую цену, ликвидность, скорости
/// (обе > 0), время, комиссию пула (0.64). Возвращает: сжатую цену. None — откат (toSqrtRatio, fullMulDiv). Ошибок нет.
pub fn next_sqrt_ratio(
    sqrt_ratio: U256,
    liquidity: u128,
    rate0: u128,
    rate1: u128,
    elapsed: u64,
    fee: u64,
) -> Option<U256> {
    let ssr = sqrt_sale_ratio(rate0, rate1);
    let fixed = to_fixed(sqrt_ratio);
    let round_up = fixed > ssr;
    let c = compute_c(fixed, ssr)?;
    if c.is_zero() || liquidity == 0 {
        return to_sqrt_ratio(ssr, round_up);
    }
    let without_fee = (U256::from(rate0) * U256::from(rate1)).root(2);
    let wf = without_fee.to::<u128>();
    let rate = U256::from(wf - compute_fee(wf, fee));
    // 12392656037 = floor(LOG2E · 2^33) — константа протокола (twamm.sol L117-L118).
    let exponent = (rate * U256::from(elapsed) * U256::from(12_392_656_037u64))
        .checked_div(U256::from(liquidity))
        .unwrap_or_default();
    if exponent >= U256::from(1u8) << 70usize {
        return to_sqrt_ratio(ssr, round_up);
    }
    let e = I256::from_raw(exp2(exponent)? << 64usize);
    let num = (e - c).unsigned_abs();
    let den = (e + c).unsigned_abs();
    let mut next = mul_div(ssr, num, den)?;
    next = if round_up {
        next.max(ssr)
    } else {
        next.min(ssr)
    };
    to_sqrt_ratio(next, round_up)
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{I256, U256};

    use super::super::ekubo_sqrt::{to_fixed, to_sqrt_ratio};
    use super::{amount_from_sale_rate, compute_c, next_sqrt_ratio, sqrt_sale_ratio};

    fn n(s: &str) -> U256 {
        U256::from_str_radix(s, 10).unwrap()
    }

    fn sh(x: u128, s: usize) -> U256 {
        U256::from(x) << s
    }

    /// Векторы официального теста: evm-contracts@e5574ab test/math/twamm.t.sol L59-L124 (twamm.sol не менялся с
    /// 61d09daa — git diff пуст; в 61d09daa файл теста пуст).
    #[test]
    fn helpers_official_vectors() {
        assert_eq!(
            amount_from_sale_rate(100 << 32, 3, false),
            U256::from(300u32)
        );
        assert_eq!(
            amount_from_sale_rate(125 << 31, 3, false),
            U256::from(187u32)
        );
        assert_eq!(
            amount_from_sale_rate(125 << 31, 3, true),
            U256::from(188u32)
        );
        assert_eq!(amount_from_sale_rate(1, 3, true), U256::from(1u32));
        let c = compute_c(sh(1, 128), sh(1, 129)).unwrap();
        assert_eq!(
            c,
            I256::from_raw(n("113427455640312821154458202477256070485"))
        );
        assert_eq!(compute_c(sh(1, 128), sh(1, 127)).unwrap(), -c);
        assert!(compute_c(sh(1, 128), sh(1, 128)).unwrap().is_zero());
        assert_eq!(sqrt_sale_ratio(1, 1), sh(1, 128));
        assert_eq!(
            sqrt_sale_ratio(100, 1),
            n("34028236692093846346337460743176821142")
        );
        assert_eq!(
            sqrt_sale_ratio(1, 100),
            n("3402823669209384634633746074317682114560")
        );
        let big = n("24519928653854221733733552434404944576644526926077100032");
        assert_eq!(sqrt_sale_ratio(1, (1u128 << 112) - 1), big);
    }

    /// Векторы test_computeNextSqrtRatio_examples / _example_from_production (twamm.t.sol L162-L308).
    #[test]
    #[rustfmt::skip]
    fn next_sqrt_ratio_official_vectors() {
        let cases: [(U256, u128, u128, u128, u64, u64, &str); 6] = [
            (sh(10_000, 128), 10_000, 458_864_027, 280_824_784, 46_800, 0, "714795237151155238153964311638230171648"),
            (sh(1, 128) / U256::from(10_000u32), 1_000_000, 707 << 32, 179 << 32, 12, ((30u128 << 64) / 10_000) as u64, "762756935888947507319423427130949632"),
            (sh(1, 128), 1_000_000, 100_000 << 32, 1 << 32, 12, 1 << 63, "212677851090737004084435068911850881024"),
            (sh(1, 128), 1_000_000, 1 << 32, 100_000 << 32, 12, 0, "748610263916272246100204618056279785472"),
            (n("286363514177267035440548892163466107483369185"), 130_385_243_018_985_227, 1_917_585_044_284, 893_194_653_345_642_013_054_241_177, 360, 922_337_203_685_477_580, "286548851173856260703560045093187956263354368"),
            (U256::from(4_182_607_738_901_102_592u128 + (148_436_996_701_757u128 << 64)), 4_472_135_213_867, 3_728_260_255_814_876_407_785, 1_597_830_095_238_095, 2688, 9_223_372_036_854_775, "75660834358443397537995245133758464"),
        ];
        for (fixed, l, r0, r1, t, fee, want) in cases {
            let sr = to_sqrt_ratio(fixed, false).unwrap();
            let got = next_sqrt_ratio(sr, l, r0, r1, t, fee).unwrap();
            assert_eq!(to_fixed(got), n(want), "{want}");
        }
    }
}
