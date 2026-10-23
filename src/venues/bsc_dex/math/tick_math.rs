//! Файл: venues/bsc_dex/math/tick_math.rs
//! Что делает: sqrt(1.0001^tick)·2^96 по тику (TickMath.getSqrtRatioAtTick) — точный порт с константами контракта.
//! Кто вызывает: math/swap_cl.rs.
//! От чего зависит: alloy-primitives.
//! Настройки: нет.
//!
//! Константы — значения протокола из исходника (CODING_STANDARDS: исключение для констант протокола):
//! https://github.com/pancakeswap/pancake-v3-contracts/blob/986847948755cba528324d41be19480731c36c2a/projects/v3-core/contracts/libraries/TickMath.sol#L8-L52
//! (у Uniswap V3/V4 и Infinity CL — те же: v4-core src/libraries/TickMath.sol, infinity-core src/pool-cl/libraries/TickMath.sol).

use alloy_primitives::U256;

/// Минимальный тик: log_1.0001(2^-128).
pub const MIN_TICK: i32 = -887_272;
/// Максимальный тик.
pub const MAX_TICK: i32 = 887_272;

/// MIN_SQRT_RATIO = getSqrtRatioAtTick(MIN_TICK).
pub fn min_sqrt_ratio() -> U256 {
    U256::from(4_295_128_739u64)
}

/// MAX_SQRT_RATIO = getSqrtRatioAtTick(MAX_TICK).
pub fn max_sqrt_ratio() -> U256 {
    U256::from_str_radix("1461446703485210103287273052203988822378723970342", 10)
        .unwrap_or(U256::MAX)
}

/// Множители по битам |tick| (128.128), начиная с бита 0x2 (TickMath.sol, строки 26–45).
const MULS: [u128; 19] = [
    0xfff97272373d413259a46990580e213a,
    0xfff2e50f5f656932ef12357cf3c7fdcc,
    0xffe5caca7e10e4e61c3624eaa0941cd0,
    0xffcb9843d60f6159c9db58835c926644,
    0xff973b41fa98c081472e6896dfb254c0,
    0xff2ea16466c96a3843ec78b326b52861,
    0xfe5dee046a99a2a811c461f1969c3053,
    0xfcbe86c7900a88aedcffc83b479aa3a4,
    0xf987a7253ac413176f2b074cf7815e54,
    0xf3392b0822b70005940c7a398e4b70f3,
    0xe7159475a2c29b7443b29c7fa6e889d9,
    0xd097f3bdfd2022b8845ad8f792aa5825,
    0xa9f746462d870fdf8a65dc1f90e061e5,
    0x70d869a156d2a1b890bb3df62baf32f7,
    0x31be135f97d08fd981231505542fcfa6,
    0x9aa508b5b7a84e1c677de54f3e99bc9,
    0x5d6af8dedb81196699c329225ee604,
    0x2216e584f5fa1ea926041bedfe98,
    0x48a170391f7dc42444e8fa2,
];

/// getSqrtRatioAtTick. Принимает: тик. Возвращает: sqrtPriceX96; None — |tick| > MAX_TICK (контракт: revert 'T').
pub fn sqrt_ratio_at_tick(tick: i32) -> Option<U256> {
    let abs = tick.unsigned_abs();
    if abs > MAX_TICK as u32 {
        return None;
    }
    let mut ratio = if abs & 1 != 0 {
        U256::from(0xfffcb933bd6fad37aa2d162d1a594001u128)
    } else {
        U256::from(1u8) << 128
    };
    for (i, m) in MULS.iter().enumerate() {
        if abs & (2 << i) != 0 {
            ratio = (ratio * U256::from(*m)) >> 128;
        }
    }
    if tick > 0 {
        ratio = U256::MAX / ratio;
    }
    let low = ratio & U256::from(u32::MAX);
    Some(
        (ratio >> 32)
            + if low.is_zero() {
                U256::ZERO
            } else {
                U256::from(1u8)
            },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Значения из официальных тестов Uniswap v3-core (test/TickMath.spec.ts#L30-L52, коммит 5ea68a77).
    #[test]
    fn official_values() {
        assert_eq!(sqrt_ratio_at_tick(MIN_TICK).unwrap(), min_sqrt_ratio());
        assert_eq!(
            sqrt_ratio_at_tick(MIN_TICK + 1).unwrap(),
            U256::from(4_295_343_490u64)
        );
        assert_eq!(
            sqrt_ratio_at_tick(MAX_TICK - 1).unwrap().to_string(),
            "1461373636630004318706518188784493106690254656249"
        );
        assert_eq!(sqrt_ratio_at_tick(MAX_TICK).unwrap(), max_sqrt_ratio());
        assert!(sqrt_ratio_at_tick(MIN_TICK - 1).is_none());
        assert!(sqrt_ratio_at_tick(MAX_TICK + 1).is_none());
        // tick 0 → 2^96 (Constants.SQRT_PRICE_1_1 в v4-core test/utils/Constants.sol).
        assert_eq!(
            sqrt_ratio_at_tick(0).unwrap().to_string(),
            "79228162514264337593543950336"
        );
    }
}
