//! Файл: venues/bsc_dex/math/fee_algebra_v1.rs
//! Что делает: решение 463 (T463-02) — комиссия, которую получит своп пула THENA Fusion (линия AlgebraV1) в СЛЕДУЮЩЕМ
//! блоке. Первый своп блока с новым временем (AlgebraPool._calculateSwapAndLock) пишет точку оракула
//! (DataStorageOperator.write → DataStorage.write: новая точка из последней с volumePerLiquidityInBlock пула) и берёт
//! комиссию _getNewFee → getFee(time, tick, index + 1, liquidity) = AdaptiveFee(средняя волатильность / 15, средний
//! объём на ликвидность) за окно WINDOW = 1 день после этой записи (DataStorage.getAverages). Время блока = времени
//! последней точки — точка не пишется, комиссия = globalState.fee. Здесь — порт этой цепочки без сети: входы — точки
//! оракула вокруг индекса, начало окна (его даёт getTimepoints узла), volumePerLiquidityInBlock, feeConfig.
//! Кто вызывает: pools/fusion_fee.rs (комиссия следующего блока при чтении пула и после Swap); тесты.
//! Ликвидность пула в комиссию не входит (в точке оракула она даёт только secondsPerLiquidityCumulative).
//! От чего зависит: math/adaptive_fee.rs, alloy-primitives (I256, U256).
//! Настройки: нет.
//!
//! Источник: код пула THENA Fusion — Sourcify 56, пул 0x1b9a1120…ef7d («полное совпадение» относится к коду ПУЛА):
//! AlgebraPool.sol (_calculateSwapAndLock: _writeTimepoint(cache.timepointIndex, blockTimestamp, cache.startTick,
//! currentLiquidity, cache.volumePerLiquidityInBlock), при новом индексе cache.fee = _getNewFee(blockTimestamp,
//! currentTick, newTimepointIndex, currentLiquidity)). Библиотека DataStorage.sol и делитель 15
//! (DataStorageOperator.getFee) — из cryptoalgebra/AlgebraV1 master (src/core/contracts/libraries/DataStorage.sol:
//! write, createNewTimepoint, _volatilityOnRange, _getAverageTick, getSingleTimepoint, getAverages,
//! lteConsideringOverflow), не из верифицированного кода оператора THENA; с оператором THENA совпали численно на
//! записи tests/data/bsc_thena_fusion_2026-11-20_t237 (60 пулов): этот порт при volumePerLiquidityInBlock = 0 = getFee
//! оператора на время блока + 0 / 1 / 3 с (180 из 180).
//! Арифметика — как solc 0.7.6 (без проверок переполнения): int56 / uint88 / uint144 / uint32 — по модулю ширины.

use alloy_primitives::{I256, U256};

use super::adaptive_fee::{FeeConfig, get_fee};

/// DataStorage.WINDOW = 1 days.
pub const WINDOW: u32 = 86_400;
/// Предел dex.fusion_fee_horizon_s (секунды): secondsAgo = WINDOW + горизонт + 1 — элемент uint32[] secondsAgos
/// getTimepoints оператора (DataStorageOperator.sol AlgebraV1). Одна константа на код (pools/fusion_fee.rs horizon) и
/// каталог настроек (settings/catalog/dex.rs FUSION_HORIZON_MAX — источник) — четвёртое ревью t236.
pub const HORIZON_MAX: u32 = crate::settings::catalog::dex::FUSION_HORIZON_MAX as u32;
// Предел каталога совпадает с протоколом: u32::MAX − WINDOW − 1 (иначе сборка не пройдёт).
const _: () = assert!(HORIZON_MAX == u32::MAX - WINDOW - 1);
/// Делитель средней волатильности в DataStorageOperator.getFee (`volatilityAverage / 15`).
const VOLATILITY_DIV: u128 = 15;

/// Точка оракула (DataStorage.Timepoint; timepoints(uint256) пула). secondsPerLiquidityCumulative комиссии не нужен.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timepoint {
    pub initialized: bool,
    pub ts: u32,
    /// tickCumulative (int56).
    pub tick_cum: i64,
    /// volatilityCumulative (uint88).
    pub vol: u128,
    /// averageTick (int24).
    pub avg_tick: i32,
    /// volumePerLiquidityCumulative (uint144).
    pub vpl: U256,
}

/// Накопители на начало окна (время − WINDOW) — getTimepoints узла на это время (до записи новой точки).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowStart {
    pub tick_cum: i64,
    pub vol: u128,
    pub vpl: U256,
}

/// Всё для комиссии следующего блока, прочитанное на одном блоке (pools/fusion_fee.rs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Oracle {
    pub cfg: FeeConfig,
    /// globalState.timepointIndex.
    pub index: u16,
    /// Точки index − 1, index, index + 1, index + 2 и 0.
    pub prev: Timepoint,
    pub last: Timepoint,
    pub next1: Timepoint,
    pub next2: Timepoint,
    pub zero: Timepoint,
    /// volumePerLiquidityInBlock пула (старшие 128 бит слота 3).
    pub vpl_in_block: u128,
    /// globalState.fee на блоке чтения.
    pub fee: u32,
    /// Начало окна по времени свопа (время, накопители).
    pub starts: Vec<(u32, WindowStart)>,
}

const M88: u128 = (1u128 << 88) - 1;

/// int56 по модулю (как solc 0.7.6). Ошибок нет.
fn i56(v: i128) -> i64 {
    ((v << 72) >> 72) as i64
}

/// int24 по модулю. Ошибок нет.
fn i24(v: i128) -> i32 {
    ((v << 104) >> 104) as i32
}

/// uint144 по модулю. Ошибок нет.
fn u144(v: U256) -> U256 {
    let m = (U256::from(1u8) << 144usize) - U256::from(1u8);
    v & m
}

/// DataStorage.lteConsideringOverflow. Ошибок нет.
fn lte(a: u32, b: u32, now: u32) -> bool {
    let res = a > now;
    if res == (b > now) {
        return a <= b;
    }
    res
}

/// (накопитель тика `a` − `b`) в int56, деленное к нулю на `d` секунд (средний тик отрезка, int24). None — d = 0.
/// Ошибок нет.
fn tick_mean(a: i64, b: i64, d: u32) -> Option<i32> {
    let diff = i128::from(i56(i128::from(a) - i128::from(b)));
    diff.checked_div(i128::from(d)).map(i24)
}

/// DataStorage._volatilityOnRange (int256, деление к нулю), итог — младшие 88 бит (uint88 в createNewTimepoint).
/// Принимает: dt, тики и средние тики слева / справа. Возвращает: волатильность; None — dt = 0. Ошибок нет.
fn volatility_on_range(dt: u32, t0: i32, t1: i32, a0: i32, a1: i32) -> Option<u128> {
    let dt = I256::try_from(i64::from(dt)).ok()?;
    let t = |v: i32| I256::try_from(i64::from(v)).ok();
    let (t0, t1, a0, a1) = (t(t0)?, t(t1)?, t(a0)?, t(a1)?);
    let one = I256::ONE;
    let two = one + one;
    let six = two * two + two;
    let k = (t1 - t0) - (a1 - a0);
    let b = (t0 - a0) * dt;
    let sum_sq = dt * (dt + one) * (two * dt + one);
    let sum_seq = dt * (dt + one);
    let num = k * k * sum_sq + six * b * k * sum_seq + six * dt * b * b;
    let v = num.checked_div(six * dt * dt)?;
    let low = v.into_raw() & U256::from(M88);
    Some(low.to::<u128>())
}

/// Средний тик при записи новой точки на время `time` (DataStorage._getAverageTick; oldest — точка index + 1, если
/// инициализирована, иначе 0). None — нужно начало окна, а его нет. Ошибок нет.
fn average_tick(o: &Oracle, time: u32, tick: i32, start: Option<&WindowStart>) -> Option<i32> {
    let (last, prev) = (&o.last, &o.prev);
    let oldest = if o.next1.initialized {
        &o.next1
    } else {
        &o.zero
    };
    let from = time.wrapping_sub(WINDOW);
    if !lte(oldest.ts, from, time) {
        if last.ts == oldest.ts {
            return Some(tick);
        }
        let d = last.ts.wrapping_sub(oldest.ts);
        return tick_mean(last.tick_cum, oldest.tick_cum, d);
    }
    if lte(last.ts, from, time) {
        if !prev.initialized {
            return Some(tick);
        }
        let d = last.ts.wrapping_sub(prev.ts);
        return tick_mean(last.tick_cum, prev.tick_cum, d);
    }
    let s = start?;
    let span = last.ts.wrapping_sub(time).wrapping_add(WINDOW);
    tick_mean(last.tick_cum, s.tick_cum, span)
}

/// Начало окна (время − WINDOW) после записи новой точки `(new_vol, new_vpl)` на время `time`: окно начинается между
/// последней и новой точкой — интерполяция getSingleTimepoint (beforeOrAt = последняя, atOrAfter = новая); иначе —
/// начало окна узла `start`. Возвращает: (волатильность, объём на ликвидность); None — начала окна нет. Ошибок нет.
fn window_start(
    o: &Oracle,
    time: u32,
    new: (u128, U256),
    start: Option<&WindowStart>,
) -> Option<(u128, U256)> {
    let last = &o.last;
    let from = time.wrapping_sub(WINDOW);
    if !lte(last.ts, from, time) {
        let s = start?;
        return Some((s.vol, s.vpl));
    }
    if from == last.ts {
        return Some((last.vol, last.vpl));
    }
    let dt = time.wrapping_sub(last.ts);
    let td = from.wrapping_sub(last.ts);
    let dv = new.0.wrapping_sub(last.vol) & M88;
    let v = last.vol.wrapping_add(dv / u128::from(dt) * u128::from(td)) & M88;
    let dp = u144(new.1.wrapping_sub(last.vpl));
    let p = u144(last.vpl + dp / U256::from(dt) * U256::from(td));
    Some((v, p))
}

/// Комиссия свопа, первого в блоке со временем `time`, при тике пула на начало блока. Принимает: оракул, время, тик.
/// Возвращает: комиссию (пипсы); None — нужного начала окна нет или параметры дают откат. Ошибок нет.
pub fn fee_at(o: &Oracle, time: u32, tick: i32) -> Option<u32> {
    let last = &o.last;
    if last.ts == time {
        return Some(o.fee);
    }
    let start = o.starts.iter().find(|(t, _)| *t == time).map(|(_, s)| s);
    let avg = average_tick(o, time, tick, start)?;
    // DataStorage.write: oldestIndex = index + 1, если инициализирована, иначе 0; prevTick — при index ≠ oldestIndex.
    let oldest_idx = if o.next1.initialized {
        o.index.wrapping_add(1)
    } else {
        0
    };
    let prev_tick = if o.index != oldest_idx {
        let d = last.ts.wrapping_sub(o.prev.ts);
        tick_mean(last.tick_cum, o.prev.tick_cum, d)?
    } else {
        tick
    };
    let dt = time.wrapping_sub(last.ts);
    let vol = volatility_on_range(dt, prev_tick, tick, last.avg_tick, avg)?;
    let new_vol = last.vol.wrapping_add(vol) & M88;
    let new_vpl = u144(last.vpl + U256::from(o.vpl_in_block));
    // getAverages после записи (индекс index + 1): oldest — index + 2, если инициализирована, иначе 0 (при
    // index + 1 = 0 точка 0 — сама новая).
    let mut zero = o.zero;
    if o.index.wrapping_add(1) == 0 {
        zero.initialized = true;
        zero.ts = time;
        zero.vol = new_vol;
        zero.vpl = new_vpl;
    }
    let oldest = if o.next2.initialized { o.next2 } else { zero };
    let from = time.wrapping_sub(WINDOW);
    let (va, pa) = if lte(oldest.ts, from, time) {
        let (sv, sp) = window_start(o, time, (new_vol, new_vpl), start)?;
        let va = (new_vol.wrapping_sub(sv) & M88) / u128::from(WINDOW);
        (va, u144(new_vpl.wrapping_sub(sp)) >> 57usize)
    } else if time != oldest.ts {
        let d = u128::from(time.wrapping_sub(oldest.ts));
        let va = (new_vol.wrapping_sub(oldest.vol) & M88) / d;
        (va, u144(new_vpl.wrapping_sub(oldest.vpl)) >> 57usize)
    } else {
        (0, U256::ZERO)
    };
    get_fee(U256::from(va / VOLATILITY_DIV), pa, &o.cfg)
}

/// Комиссия следующего блока после блока чтения со временем `block_ts`: следующий блок BSC — с тем же временем в
/// секундах или на секунду позже (запись t237: 2 блока в секунду), какой из двух — заранее неизвестно, поэтому берётся
/// большая из двух (котировка не выше той, что даст контракт). Принимает: оракул, время блока чтения, тик на конец
/// блока. Возвращает: комиссию (пипсы); None — одна из двух не считается. Ошибок нет.
pub fn next_block_fee(o: &Oracle, block_ts: u32, tick: i32) -> Option<u32> {
    let a = fee_at(o, block_ts, tick)?;
    let b = fee_at(o, block_ts.wrapping_add(1), tick)?;
    Some(a.max(b))
}
