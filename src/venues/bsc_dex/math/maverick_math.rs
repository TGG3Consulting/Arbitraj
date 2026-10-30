//! Файл: venues/bsc_dex/math/maverick_math.rs
//! Что делает: базовая фикс-точечная математика Maverick V2 (решения 80, 73 — Maverick назван в охвате экзотической
//! математики) — порт официальной библиотеки контракта `Math.sol` на целых U256 (фикс-точка D18: ONE=10^18), с теми же
//! округлениями и теми же местами отката, что в Solidity 0.8.25. Чистая математика без сети.
//! ВАЖНО (решение 23): различаем два умножения-с-делением Maverick, ровно как контракт. `OzMath.mulDiv` (полное
//! 512-битное промежуточное, откат при результате ≥ 2^256) → full_math::mul_div / mul_div_up. `mulDivDown` / `mulDivUp`
//! (одинарной ширины: `x·y` откатывается при переполнении 2^256, делитель-ноль трактуется как 1) → checked_mul плюс
//! замена нулевого делителя. Блоки `unchecked` контракта (сложение/вычитание, `ONE·…`, `…·1e9`) → wrapping_*.
//! Где контракт откатился бы (переполнение mulDiv, деление на ноль в inv, revert каста int32) — None.
//! Кто вызывает: math/maverick_tick_math.rs (цена тика, L, цена по резервам); тесты (tests/bsc_dex_maverick_math.rs).
//! От чего зависит: math/full_math.rs (512-битный mulDiv), alloy-primitives (U256, I256). Настройки: нет.
//!
//! Источник (официальный, решение 23): maverickprotocol/v2-common (HEAD d91e8714a20b65913166d6b6608590ff822c3f06):
//! contracts/libraries/Math.sol; ONE / ONE_SQUARED / DEFAULT_DECIMALS / DEFAULT_SCALE — contracts/libraries/Constants.sol.

use alloy_primitives::{I256, U256};

use super::full_math as fm;

/// Constants.ONE = 10^18 (единица фикс-точки D18). Ошибок нет.
pub fn one() -> U256 {
    U256::from(10u8).pow(U256::from(18u8))
}

/// Constants.ONE_SQUARED = 10^36. Ошибок нет.
pub fn one_squared() -> U256 {
    U256::from(10u8).pow(U256::from(36u8))
}

/// Math.min (uint256). Ошибок нет.
pub fn min(x: U256, y: U256) -> U256 {
    x.min(y)
}
/// Math.max (uint256). Ошибок нет.
pub fn max(x: U256, y: U256) -> U256 {
    x.max(y)
}
/// Math.min (int256). Ошибок нет.
pub fn min_i(x: I256, y: I256) -> I256 {
    x.min(y)
}
/// Math.max (int256). Ошибок нет.
pub fn max_i(x: I256, y: I256) -> I256 {
    x.max(y)
}
/// Math.min128 (uint128). Ошибок нет.
pub fn min128(x: u128, y: u128) -> u128 {
    x.min(y)
}
/// Math.max128 (uint128). Ошибок нет.
pub fn max128(x: u128, y: u128) -> u128 {
    x.max(y)
}

/// Math.clip: x<y ? 0 : x−y. Ошибок нет.
pub fn clip(x: U256, y: U256) -> U256 {
    if x < y { U256::ZERO } else { x - y }
}
/// Math.clip128. Ошибок нет.
pub fn clip128(x: u128, y: u128) -> u128 {
    x.saturating_sub(y)
}

/// Math.boundValue: min(max(value, lo), hi). Ошибок нет.
pub fn bound_value(value: U256, lo: U256, hi: U256) -> U256 {
    min(max(value, lo), hi)
}

/// Math.mulDivFloor: floor(x·y / max(1,k)) полным 512-битным mulDiv. None — результат ≥ 2^256.
pub fn mul_div_floor(x: U256, y: U256, k: U256) -> Option<U256> {
    fm::mul_div(x, y, max(U256::from(1u8), k))
}
/// Math.mulDivCeil: ceil(x·y / max(1,k)). None — результат ≥ 2^256.
pub fn mul_div_ceil(x: U256, y: U256, k: U256) -> Option<U256> {
    fm::mul_div_up(x, y, max(U256::from(1u8), k))
}
/// Math.mulFloor: floor(x·y / ONE). None — результат ≥ 2^256.
pub fn mul_floor(x: U256, y: U256) -> Option<U256> {
    fm::mul_div(x, y, one())
}
/// Math.mulCeil: ceil(x·y / ONE). None — результат ≥ 2^256.
pub fn mul_ceil(x: U256, y: U256) -> Option<U256> {
    fm::mul_div_up(x, y, one())
}
/// Math.divFloor: floor(x·ONE / max(1,y)). None — результат ≥ 2^256.
pub fn div_floor(x: U256, y: U256) -> Option<U256> {
    mul_div_floor(x, one(), y)
}
/// Math.divCeil: ceil(x·ONE / max(1,y)). None — результат ≥ 2^256.
pub fn div_ceil(x: U256, y: U256) -> Option<U256> {
    mul_div_ceil(x, one(), y)
}

/// Math.mulDivDown: z=x·y (откат при ≥ 2^256), делитель 0 → 1, floor. None — переполнение x·y.
pub fn mul_div_down(x: U256, y: U256, denom: U256) -> Option<U256> {
    let z = x.checked_mul(y)?;
    Some(z / nz1(denom))
}
/// Math.mulDivUp: как mulDivDown, но округление вверх; z=0 → 0. None — переполнение x·y.
pub fn mul_div_up(x: U256, y: U256, denom: U256) -> Option<U256> {
    let z = x.checked_mul(y)?;
    if z.is_zero() {
        return Some(U256::ZERO);
    }
    Some((z - U256::from(1u8)) / nz1(denom) + U256::from(1u8))
}
/// Math.mulDown: floor(x·y / ONE) одинарной ширины. None — переполнение x·y.
pub fn mul_down(x: U256, y: U256) -> Option<U256> {
    mul_div_down(x, y, one())
}
/// Math.divDown: floor(x·ONE / y) одинарной ширины (y=0 → делитель 1). None — переполнение x·ONE.
pub fn div_down(x: U256, y: U256) -> Option<U256> {
    mul_div_down(x, one(), y)
}
/// Math.divUp: ceil(x·ONE / y) одинарной ширины. None — переполнение x·ONE.
pub fn div_up(x: U256, y: U256) -> Option<U256> {
    mul_div_up(x, one(), y)
}

/// Math.invFloor: floor(ONE² / x). None — x = 0 (контракт откатился бы делением на ноль).
pub fn inv_floor(x: U256) -> Option<U256> {
    if x.is_zero() {
        None
    } else {
        Some(one_squared() / x)
    }
}
/// Math.invCeil: floor((ONE²−1)/x) + 1. None — x = 0.
pub fn inv_ceil(x: U256) -> Option<U256> {
    if x.is_zero() {
        return None;
    }
    Some((one_squared() - U256::from(1u8)) / x + U256::from(1u8))
}

/// Math.sqrt: целочисленный корень вниз (solmate FixedPointMathLib). sqrt(0)=0. Ошибок нет.
pub fn sqrt(x: U256) -> U256 {
    if x.is_zero() {
        return U256::ZERO;
    }
    let mut y = x;
    let mut z = U256::from(181u64);
    if y >= (U256::from(1u8) << 136) {
        y >>= 128;
        z <<= 64;
    }
    if y >= (U256::from(1u8) << 72) {
        y >>= 64;
        z <<= 32;
    }
    if y >= (U256::from(1u8) << 40) {
        y >>= 32;
        z <<= 16;
    }
    if y >= (U256::from(1u8) << 24) {
        y >>= 16;
        z <<= 8;
    }
    z = z.wrapping_mul(y + U256::from(65536u64)) >> 18;
    for _ in 0..7 {
        z = z.wrapping_add(x / z) >> 1;
    }
    if x / z < z {
        z -= U256::from(1u8);
    }
    z
}

/// Math.abs32: |x| как uint32 (abs(int32::MIN) = 2^31). Ошибок нет.
pub fn abs32(x: i32) -> u32 {
    x.unsigned_abs()
}
/// Math.abs: |x| как uint256 (abs(int256::MIN) = 2^255). Ошибок нет.
pub fn abs(x: I256) -> U256 {
    x.unsigned_abs()
}

/// Math.scale: множитель перевода токен→D18 по числу знаков (домен decimals ≤ 18). Ошибок нет.
pub fn scale(decimals: u8) -> U256 {
    if decimals == 18 {
        U256::from(1u8)
    } else {
        U256::from(10u8).pow(U256::from(18 - decimals))
    }
}
/// Math.tokenScaleToAmmScale: amount·scaleFactor (scaleFactor=1 → amount). Ошибок нет в домене D18.
pub fn token_scale_to_amm_scale(amount: U256, scale_factor: U256) -> U256 {
    if scale_factor == U256::from(1u8) {
        amount
    } else {
        amount * scale_factor
    }
}
/// Math.ammScaleToTokenScale: D18→токен, вниз или вверх (scaleFactor=1 или amount=0 → amount). Ошибок нет в домене.
pub fn amm_scale_to_token_scale(amount: U256, scale_factor: U256, ceil: bool) -> U256 {
    if scale_factor == U256::from(1u8) || amount.is_zero() {
        return amount;
    }
    if !ceil {
        amount / scale_factor
    } else {
        (amount - U256::from(1u8)) / scale_factor + U256::from(1u8)
    }
}

/// Math.floorD8Unchecked: floor D8-числа как int32 (каст усечением, как в контракте). None — контракт откатился бы на
/// `val32 − 1` (вход ровно int32::MIN·1e8 с дробной частью).
pub fn floor_d8_unchecked(val: I256) -> Option<i32> {
    let one_d8 = I256::try_from(100_000_000i64).unwrap();
    let q = val / one_d8;
    let val32 = q.into_raw().as_limbs()[0] as u32 as i32;
    let check = val.is_negative() && !(val % one_d8).is_zero();
    if check {
        val32.checked_sub(1)
    } else {
        Some(val32)
    }
}

/// Делитель 0 → 1 (как ассемблерные mulDivDown/Up контракта). Ошибок нет.
fn nz1(d: U256) -> U256 {
    if d.is_zero() { U256::from(1u8) } else { d }
}
