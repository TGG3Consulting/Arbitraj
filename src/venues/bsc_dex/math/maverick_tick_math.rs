//! Файл: venues/bsc_dex/math/maverick_tick_math.rs
//! Что делает: тиковая математика Maverick V2 (решения 80, 73) — порт официальной библиотеки контракта `TickMath.sol`
//! на целых U256, с теми же округлениями и теми же местами отката, что в Solidity 0.8.25. Чистая математика без сети.
//! Что здесь: `tick_sqrt_price` (sqrt-цена края тика по тикспейсингу, D18) и `tick_sqrt_prices` (нижняя и верхняя
//! цены тика); `get_tick_l` (ликвидность тика по его резервам A/B и ценам краёв), `get_sqrt_price` (sqrt-цена внутри
//! тика по резервам и L) и `get_tick_sqrt_price_and_l` (оба). Эти три — «дельты» Maverick: как из резервов бина и
//! границ получаются L и текущая цена (именно их возьмёт будущий расчёт свопа по бинам).
//! Фикс-точка Maverick — цена 1.0001^tick в D18; константы магического разложения и предел MAX_TICK=322378 (цена 1e14)
//! взяты дословно у контракта. `unchecked`-блоки контракта → wrapping_* (ровно те же переполнения mod 2^256).
//! ВАЖНО (решение 23): здесь только математика. Чтение состояния пула/бинов, события, квотер, граф, и сам цикл свопа
//! Maverick (прохода по бинам НЕТ в v2-common — он в контракте пула v2-core, см. README) — отдельная следующая задача.
//! Кто вызывает: будущий расчёт свопа Maverick и котировка (фаза B); тесты (tests/bsc_dex_maverick_math.rs).
//! От чего зависит: math/maverick_math.rs, math/full_math.rs, alloy-primitives (U256).
//! Настройки: нет (резервы, тикспейсинг, границы приходят с контракта пула в фазе B).
//!
//! Источник (официальный, решение 23): maverickprotocol/v2-common (HEAD d91e8714a20b65913166d6b6608590ff822c3f06):
//! contracts/libraries/TickMath.sol (tickSqrtPrice / subTickIndex / tickSqrtPrices / getTickL / getSqrtPrice /
//! getTickSqrtPriceAndL); MAX_TICK — contracts/libraries/Constants.sol.

use alloy_primitives::U256;

use super::full_math as fm;
use super::maverick_math::{bound_value, div_down, mul_div_down, mul_down, mul_floor, one, sqrt};

/// Constants.MAX_TICK = 322378 (макс. цена 1e14 в D18).
const MAX_TICK: u32 = 322_378;

/// TickMath.subTickIndex: |tick|·uint32(tickSpacing), арифметика uint32. None — откат (переполнение uint32 либо
/// subTick > MAX_TICK, т.е. TickMaxExceeded).
fn sub_tick_index(tick_spacing: U256, tick: i32) -> Option<u32> {
    let ts = tick_spacing.as_limbs()[0] as u32; // uint32(tickSpacing): младшие 32 бита
    let sub = tick.unsigned_abs().checked_mul(ts)?;
    if sub > MAX_TICK { None } else { Some(sub) }
}

/// TickMath.tickSqrtPrice: sqrt-цена края тика (D18). None — откат subTickIndex (тик за пределом).
pub fn tick_sqrt_price(tick_spacing: U256, tick: i32) -> Option<U256> {
    let t = U256::from(sub_tick_index(tick_spacing, tick)?);
    let bit = |m: u64| !(t & U256::from(m)).is_zero();
    let mut ratio = if bit(0x1) {
        U256::from(0xfffcb933bd6fad9d3af5f0b9f25db4d6u128)
    } else {
        U256::from(1u8) << 128
    };
    let step = |r: U256, c: u128| r.wrapping_mul(U256::from(c)) >> 128;
    if bit(0x2) {
        ratio = step(ratio, 0xfff97272373d41fd789c8cb37ffcaa1c);
    }
    if bit(0x4) {
        ratio = step(ratio, 0xfff2e50f5f656ac9229c67059486f389);
    }
    if bit(0x8) {
        ratio = step(ratio, 0xffe5caca7e10e81259b3cddc7a064941);
    }
    if bit(0x10) {
        ratio = step(ratio, 0xffcb9843d60f67b19e8887e0bd251eb7);
    }
    if bit(0x20) {
        ratio = step(ratio, 0xff973b41fa98cd2e57b660be99eb2c4a);
    }
    if bit(0x40) {
        ratio = step(ratio, 0xff2ea16466c9838804e327cb417cafcb);
    }
    if bit(0x80) {
        ratio = step(ratio, 0xfe5dee046a99d51e2cc356c2f617dbe0);
    }
    if bit(0x100) {
        ratio = step(ratio, 0xfcbe86c7900aecf64236ab31f1f9dcb5);
    }
    if bit(0x200) {
        ratio = step(ratio, 0xf987a7253ac4d9194200696907cf2e37);
    }
    if bit(0x400) {
        ratio = step(ratio, 0xf3392b0822b88206f8abe8a3b44dd9be);
    }
    if bit(0x800) {
        ratio = step(ratio, 0xe7159475a2c578ef4f1d17b2b235d480);
    }
    if bit(0x1000) {
        ratio = step(ratio, 0xd097f3bdfd254ee83bdd3f248e7e785e);
    }
    if bit(0x2000) {
        ratio = step(ratio, 0xa9f746462d8f7dd10e744d913d033333);
    }
    if bit(0x4000) {
        ratio = step(ratio, 0x70d869a156ddd32a39e257bc3f50aa9b);
    }
    if bit(0x8000) {
        ratio = step(ratio, 0x31be135f97da6e09a19dc367e3b6da40);
    }
    if bit(0x10000) {
        ratio = step(ratio, 0x9aa508b5b7e5a9780b0cc4e25d61a56);
    }
    if bit(0x20000) {
        ratio = step(ratio, 0x5d6af8dedbcb3a6ccb7ce618d14225);
    }
    if bit(0x40000) {
        ratio = step(ratio, 0x2216e584f630389b2052b8db590e);
    }
    if tick > 0 {
        ratio = U256::MAX / ratio;
    }
    Some(ratio.wrapping_mul(one()) >> 128)
}

/// TickMath.tickSqrtPrices: (нижняя, верхняя) sqrt-цены тика = (tickSqrtPrice(tick), tickSqrtPrice(tick+1)). None —
/// откат любого из краёв.
pub fn tick_sqrt_prices(tick_spacing: U256, tick: i32) -> Option<(U256, U256)> {
    Some((
        tick_sqrt_price(tick_spacing, tick)?,
        tick_sqrt_price(tick_spacing, tick + 1)?,
    ))
}

/// TickMath.getTickL: ликвидность тика по резервам A/B и sqrt-ценам краёв. None — там, где OzMath.mulDiv контракта
/// откатился бы (результат ≥ 2^256).
pub fn get_tick_l(
    mut reserve_a: U256,
    mut reserve_b: U256,
    sqrt_lower: U256,
    sqrt_upper: U256,
) -> Option<U256> {
    let diff = sqrt_upper.wrapping_sub(sqrt_lower);
    let mut bump = 0usize;
    if (reserve_a >> 78usize).is_zero() && (reserve_b >> 78usize).is_zero() {
        bump = 57;
        reserve_a <<= 57usize;
        reserve_b <<= 57usize;
    }
    if reserve_b.is_zero() {
        return Some(div_down(reserve_a, diff)? >> bump);
    }
    if reserve_a.is_zero() {
        return Some(mul_div_down(mul_down(reserve_b, sqrt_lower)?, sqrt_upper, diff)? >> bump);
    }
    let b = div_down(reserve_a, sqrt_upper)?.wrapping_add(mul_down(reserve_b, sqrt_lower)?) >> 1;
    let inner = fm::mul_div(b, b, one())?.wrapping_add(fm::mul_div(
        mul_floor(reserve_b, reserve_a)?,
        diff,
        sqrt_upper,
    )?);
    let root = sqrt(inner).wrapping_mul(U256::from(1_000_000_000u64));
    Some(fm::mul_div(b.wrapping_add(root), sqrt_upper, diff)? >> bump)
}

/// TickMath.getSqrtPrice: sqrt-цена внутри тика по резервам и L (пустой край → граница). None — там, где divDown
/// контракта откатился бы (переполнение x·ONE).
pub fn get_sqrt_price(
    reserve_a: U256,
    reserve_b: U256,
    sqrt_lower: U256,
    sqrt_upper: U256,
    liquidity: U256,
) -> Option<U256> {
    if reserve_a.is_zero() {
        return Some(sqrt_lower);
    }
    if reserve_b.is_zero() {
        return Some(sqrt_upper);
    }
    let num = reserve_a.wrapping_add(mul_down(liquidity, sqrt_lower)?);
    let den = reserve_b.wrapping_add(div_down(liquidity, sqrt_upper)?);
    let sp = sqrt(one().wrapping_mul(div_down(num, den)?));
    Some(bound_value(sp, sqrt_lower, sqrt_upper))
}

/// TickMath.getTickSqrtPriceAndL: (sqrt-цена, L) по резервам и границам. None — откат getTickL/getSqrtPrice.
pub fn get_tick_sqrt_price_and_l(
    reserve_a: U256,
    reserve_b: U256,
    sqrt_lower: U256,
    sqrt_upper: U256,
) -> Option<(U256, U256)> {
    let liquidity = get_tick_l(reserve_a, reserve_b, sqrt_lower, sqrt_upper)?;
    let sqrt_price = get_sqrt_price(reserve_a, reserve_b, sqrt_lower, sqrt_upper, liquidity)?;
    Some((sqrt_price, liquidity))
}
