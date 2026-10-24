//! Файл: venues/bsc_dex/math/bin_price.rs
//! Что делает: цена бина Infinity Bin — число 128.128 (price = base^(id − 2^23), base = 1 + binStep/10_000),
//! точный порт PriceHelper.getPriceFromId и Uint128x128Math.pow (умножения по модулю 2^256, как в EVM).
//! Кто вызывает: pools/load_bin.rs (глубина полосы бинов в USDT), math/bin_swap.rs (шаг 1c).
//! От чего зависит: alloy-primitives (U256).
//! Настройки: нет.
//!
//! Источник: https://github.com/pancakeswap/infinity-core/blob/3c5ef1be1f71442d8bc60cc6fd11f9c677b714df/src/pool-bin/libraries/PriceHelper.sol#L16-L58 ,
//! src/pool-bin/libraries/math/Uint128x128Math.sol#L87-L156, src/pool-bin/libraries/Constants.sol (SCALE_OFFSET = 128,
//! BASIS_POINT_MAX = 10_000). Документация: price = (1 + binStep/10_000)^(activeId − 8388608) —
//! developer.pancakeswap.finance, Liquidity Book (research/flow/bsc_v4.md, раздел 4).

use alloy_primitives::U256;

/// SCALE_OFFSET: число 128.128.
pub const SCALE_OFFSET: usize = 128;
/// BASIS_POINT_MAX: binStep в сотых долях процента.
const BASIS_POINT_MAX: u64 = 10_000;
/// REAL_ID_SHIFT = 2^23: id, у которого цена 1.
const REAL_ID_SHIFT: i64 = 1 << 23;
/// Предел |y| в pow: 0x100000 (дальше контракт не считает — результат 0 и откат).
const POW_LIMIT: u64 = 0x10_0000;

/// SCALE = 1 << 128.
pub fn scale() -> U256 {
    U256::from(1u8) << SCALE_OFFSET
}

/// getBase: SCALE + (binStep << 128) / 10_000. Ошибок нет.
pub fn base(bin_step: u32) -> U256 {
    scale() + (U256::from(bin_step) << SCALE_OFFSET) / U256::from(BASIS_POINT_MAX)
}

/// Цена бина 128.128 (сколько Y за 1 X в минимальных единицах, умноженное на 2^128).
/// Принимает: id бина, binStep. Возвращает: цену; None — там, где контракт откатился бы (PowUnderflow).
pub fn price_from_id(id: u32, bin_step: u32) -> Option<U256> {
    pow(base(bin_step), i64::from(id) - REAL_ID_SHIFT)
}

/// Uint128x128Math.pow(x, y) — возведение 128.128 в целую степень, побитово, как в контракте.
/// Возвращает: результат; None — результат 0 (контракт: Uint128x128Math__PowUnderflow).
pub fn pow(x: U256, y: i64) -> Option<U256> {
    if y == 0 {
        return Some(scale());
    }
    let mut invert = y < 0;
    let abs_y = y.unsigned_abs();
    let mut result = U256::ZERO;
    if abs_y < POW_LIMIT {
        result = scale();
        let mut squared = x;
        if x > U256::from(u128::MAX) {
            squared = U256::MAX / squared;
            invert = !invert;
        }
        for bit in 0..20u32 {
            if abs_y & (1 << bit) != 0 {
                result = result.wrapping_mul(squared) >> SCALE_OFFSET;
            }
            squared = squared.wrapping_mul(squared) >> SCALE_OFFSET;
        }
    }
    if result.is_zero() {
        return None;
    }
    Some(if invert { U256::MAX / result } else { result })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// id = 2^23 — цена ровно 1 (SCALE), по определению REAL_ID_SHIFT.
    #[test]
    fn id_shift_is_one() {
        assert_eq!(price_from_id(1 << 23, 25), Some(scale()));
    }

    /// Официальный тест PriceHelper.t.sol::test_Price (infinity-core 3c5ef1be, test/pool-bin/libraries/PriceHelper.t.sol#L80-L101):
    /// цена соседних id лежит по обе стороны от ожидаемой.
    #[test]
    fn official_test_price() {
        // convertDecimalPriceTo128x128(p) = (p << 128) / 1e18 (shiftDivRoundDown).
        let conv = |p: U256| (p << SCALE_OFFSET) / U256::from(1_000_000_000_000_000_000u128);
        let e18 = U256::from(1_000_000_000_000_000_000u128);
        let cases = [
            (8_574_931u32, 1u32, conv(U256::from(123_456_789u64) * e18)),
            (8_252_553, 1, conv(U256::from(1_234_567_890_000u64))),
            (8_392_773, 100, conv(e18 * e18)),
            (8_389_042, 1000, conv(e18 * e18)),
        ];
        for (id, step, expected) in cases {
            assert!(price_from_id(id - 1, step).unwrap() <= expected, "id {id}");
            assert!(price_from_id(id + 1, step).unwrap() >= expected, "id {id}");
        }
    }
}
