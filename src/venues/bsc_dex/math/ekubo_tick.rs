//! Файл: venues/bsc_dex/math/ekubo_tick.rs
//! Что делает: тик ↔ сжатая цена Ekubo (решение 447) — точный порт tickToSqrtRatio (умножение на константы по битам
//! тика) и sqrtRatioToTick (log2 рядом atanh с границами ошибки и уточнением через tickToSqrtRatio). Тик Ekubo — шаг
//! цены 1.000001 (sqrtRatio = sqrt(1.000001^tick)).
//! Кто вызывает: math/ekubo_swap.rs, math/ekubo_bitmap.rs, pools/events_ekubo.rs.
//! От чего зависит: alloy-primitives, math/ekubo_sqrt.rs.
//! Настройки: нет.
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa: src/math/ticks.sol L22-L80 (tickToSqrtRatio; константы — строки
//! L30-L71, переписаны из файла скриптом без правки), L99-L164 (sqrtRatioToTick; константы L82-L93); src/math/constants.sol L10, L14
//! (MIN_TICK, MAX_TICK). Деление со знаком «>>» Solidity для int256 — арифметический сдвиг (вниз к −∞).

use alloy_primitives::U256;

use super::ekubo_sqrt::{to_fixed, to_sqrt_ratio};

/// MIN_TICK / MAX_TICK (constants.sol L10, L14).
pub const MIN_TICK: i32 = -88_722_835;
pub const MAX_TICK: i32 = 88_722_835;

/// Множитель бита 0 (ticks.sol L30).
const BIT0: u128 = 0x8637b66cd638344daef276cd7c5;

/// (бит тика, множитель 128.128) — ticks.sol L34-L71.
const MULS: [(u32, u128); 26] = [
    (0x2, 0xffffef390978c398134b4ff3764fe410),
    (0x4, 0xffffde72140b00a354bd3dc828e976c9),
    (0x8, 0xffffbce42c7be6c998ad6318193c0b18),
    (0x10, 0xffff79c86a8f6150a32d9778eceef97c),
    (0x20, 0xfffef3911b7cff24ba1b3dbb5f8f5974),
    (0x40, 0xfffde72350725cc4ea8feece3b5f13c8),
    (0x80, 0xfffbce4b06c196e9247ac87695d53c60),
    (0x100, 0xfff79ca7a4d1bf1ee8556cea23cdbaa5),
    (0x200, 0xffef3995a5b6a6267530f207142a5764),
    (0x400, 0xffde7444b28145508125d10077ba83b8),
    (0x800, 0xffbceceeb791747f10df216f2e53ec57),
    (0x1000, 0xff79eb706b9a64c6431d76e63531e929),
    (0x2000, 0xfef41d1a5f2ae3a20676bec6f7f9459a),
    (0x4000, 0xfde95287d26d81bea159c37073122c73),
    (0x8000, 0xfbd701c7cbc4c8a6bb81efd232d1e4e7),
    (0x10000, 0xf7bf5211c72f5185f372aeb1d48f937e),
    (0x20000, 0xefc2bf59df33ecc28125cf78ec4f167f),
    (0x40000, 0xe08d35706200796273f0b3a981d90cfd),
    (0x80000, 0xc4f76b68947482dc198a48a54348c4ed),
    (0x100000, 0x978bcb9894317807e5fa4498eee7c0fa),
    (0x200000, 0x59b63684b86e9f486ec54727371ba6ca),
    (0x400000, 0x1f703399d88f6aa83a28b22d4a1f56e3),
    (0x800000, 0x3dc5dac7376e20fc8679758d1bcdcfc),
    (0x1000000, 0xee7e32d61fdb0a5e622b820f681d0),
    (0x2000000, 0xde2ee4bc381afa7089aa84bb66),
    (0x4000000, 0xc0d55d4d7152c25fb139),
];

/// K_2_OVER_LN2_X64 (ticks.sol L87), INV_LB_X64 (L90).
const K_2_OVER_LN2_X64: u128 = 53_226_052_391_377_289_966;
const INV_LB_X64: u128 = 25_572_630_076_711_825_471_857_579;

/// tickToSqrtRatio (ticks.sol L22-L80). Принимает: тик. Возвращает: сжатую цену. None — InvalidTick (|тик| > MAX_TICK).
pub fn to_sqrt_ratio_at(tick: i32) -> Option<U256> {
    let t = tick.unsigned_abs();
    if t > MAX_TICK as u32 {
        return None;
    }
    let mut ratio = (U256::from(1u8) << 128usize) - U256::from(if t & 1 == 1 { BIT0 } else { 0 });
    for (bit, m) in MULS {
        if t & bit != 0 {
            ratio = (ratio * U256::from(m)) >> 128usize;
        }
    }
    if tick > 0 {
        ratio = U256::MAX / ratio;
    }
    to_sqrt_ratio(ratio, false)
}

/// Знаковое число как (отрицательное, модуль). Пол деления на 2^128 (арифметический сдвиг). Ошибок нет.
fn floor_shr128(neg: bool, mag: U256) -> i64 {
    let q = mag >> 128usize;
    if !neg {
        return q.to::<i64>();
    }
    let exact = (mag & ((U256::from(1u8) << 128usize) - U256::from(1u8))).is_zero();
    -(q.to::<i64>() + i64::from(!exact))
}

/// Сумма знаковых (отрицательное, модуль) a + b. Ошибок нет.
fn add_signed(a: (bool, U256), b: (bool, U256)) -> (bool, U256) {
    if a.0 == b.0 {
        return (a.0, a.1 + b.1);
    }
    if a.1 >= b.1 {
        (a.0 && a.1 != b.1, a.1 - b.1)
    } else {
        (b.0, b.1 - a.1)
    }
}

/// sqrtRatioToTick (ticks.sol L99-L164). Принимает: сжатую цену (допустимую). Возвращает: тик. None — цена ноль.
pub fn tick_at(sqrt: U256) -> Option<i32> {
    let fixed = to_fixed(sqrt);
    if fixed.is_zero() {
        return None;
    }
    let negative = (fixed >> 128usize).is_zero();
    let mut x = if negative { U256::MAX / fixed } else { fixed };
    let hi = x >> 128usize;
    let msb_high = 255 - hi.leading_zeros();
    x >>= msb_high + 1;
    let one = U256::from(1u8) << 127usize;
    let a = x - one;
    let b = x + one;
    let y = (a << 127usize) / b;
    let y2 = (y * y) >> 127usize;
    let y3 = (y * y2) >> 127usize;
    let y5 = (y3 * y2) >> 127usize;
    let y7 = (y5 * y2) >> 127usize;
    let y9 = (y7 * y2) >> 127usize;
    let y11 = (y9 * y2) >> 127usize;
    let y13 = (y11 * y2) >> 127usize;
    let y15 = (y13 * y2) >> 127usize;
    let n = |v: u8| U256::from(v);
    let s =
        y + y3 / n(3) + y5 / n(5) + y7 / n(7) + y9 / n(9) + y11 / n(11) + y13 / n(13) + y15 / n(15);
    let frac = (s * U256::from(K_2_OVER_LN2_X64)) >> 127usize;
    let log2 = (U256::from(msb_high) << 64usize) + frac;
    let base = (negative, log2 * U256::from(INV_LB_X64));
    // ERROR_BOUNDS_X128 = 2^128 / 485 (ticks.sol L93).
    let eb = (U256::from(1u8) << 128usize) / U256::from(485u16);
    let lo = add_signed(base, (true, eb));
    let hi_v = add_signed(base, (false, eb));
    let tick_low = floor_shr128(lo.0, lo.1) as i32;
    let mut tick = floor_shr128(hi_v.0, hi_v.1) as i32;
    if tick != tick_low && to_sqrt_ratio_at(tick)? > sqrt {
        tick = tick_low;
    }
    Some(tick)
}

#[cfg(test)]
mod tests {
    use super::super::ekubo_sqrt::{MAX_SQRT_RATIO, MIN_SQRT_RATIO};
    use super::*;

    /// Векторы официальных тестов ядра (EkuboProtocol/evm-contracts@61d09daa test/math/ticks.t.sol L15-L17, L41-L48,
    /// L65-L71): tickToSqrtRatio(MIN_TICK / MAX_TICK) = MIN_SQRT_RATIO / MAX_SQRT_RATIO и их 64.128, тик 0 — 1.0,
    /// пример −18129342, sqrtRatioToTick(MIN_SQRT_RATIO) = MIN_TICK.
    #[test]
    fn bounds_and_zero() {
        let (min, max) = (U256::from(MIN_SQRT_RATIO), U256::from(MAX_SQRT_RATIO));
        assert_eq!(to_sqrt_ratio_at(MIN_TICK), Some(min));
        assert_eq!(to_sqrt_ratio_at(MAX_TICK), Some(max));
        let min_fixed = U256::from(18_447_191_164_202_170_524u128);
        assert_eq!(to_fixed(min), min_fixed);
        let ex = to_sqrt_ratio_at(-18_129_342).map(to_fixed);
        let want = U256::from(39_364_507_096_818_414_277_565_152_436_944_896u128);
        assert_eq!(ex, Some(want));
        assert_eq!(tick_at(min), Some(MIN_TICK));
        let one = U256::from((1u128 << 95) + (1u128 << 62));
        assert_eq!(to_sqrt_ratio_at(0), Some(one));
        assert_eq!(tick_at(one), Some(0));
        assert_eq!(to_sqrt_ratio_at(MAX_TICK + 1), None);
    }
}
