//! Файл: venues/bsc_dex/math/swap_elastic.rs
//! Что делает: цикл обмена в пуле KyberSwap Elastic (решение 81) по локальной карте тиков — порт Pool.swap из
//! официального контракта ks-elastic-sc. Идёт шагами (math/swap_elastic_step::compute_swap_step): следующий
//! инициализированный тик в сторону обмена → шаг → пересечение тика. Отличия от Uniswap V3 (math/swap_cl.rs):
//! ликвидность шага L = baseL + reinvestL; reinvestL прирастает комиссией каждого шага (deltaL) и переживает тики;
//! шаг ограничен MAX_TICK_DISTANCE (480 тиков — так держится допущение ~5% движения цены в calcReachAmount); при
//! пересечении инициализированного тика baseL += liquidityNet (вверх) / −= (вниз). TickMath у Elastic идентичен
//! Uniswap V3 — переиспользуем math/tick_math.rs. Шаг-математика (формулы) — в math/swap_elastic_step.rs, оттуда же
//! переэкспортируется для котировки и тестов.
//! ЭТОТ модуль — только математика (без сети): чтение состояния/тиков, события, исполнение — фазы 81b/81c.
//! Кто вызывает: quote/pool_quote.rs (через 81b); тесты.
//! От чего зависит: math/swap_elastic_step.rs (шаг), math/tick_math.rs, pools/ticks.rs (TickMap). Свой поиск
//! следующего тика (next_init), НЕ swap_cl::next_tick: у Elastic reinvestL прирастает на каждом шаге, поэтому точки
//! дробления обязаны быть ровно контрактными (инициализированные тики и MAX_TICK_DISTANCE), а не границы слов
//! битмапа — иначе ΔL-компаундинг разойдётся с QuoterV2.
//! Настройки: нет (комиссия пула feeUnits передаётся вызывающим — читается с контракта в 81b).
//!
//! Источники (официальные, решение 23): KyberNetwork/ks-elastic-sc (HEAD 4ab08c0): цикл swap и tempNextTick-ограничение
//! MAX_TICK_DISTANCE — contracts/Pool.sol; _updateLiquidityAndCrossTick (liquidityNet к baseL: +net вверх, −net вниз),
//! _getInitialSwapData (nextTick: вверх строго выше, вниз ≤ текущего) — contracts/PoolTicksState.sol.

use alloy_primitives::{I256, U256};

use super::super::pools::ticks::TickMap;
use super::swap_elastic_step::{abs, rev_to_int};
use super::tick_math::{MAX_TICK, MIN_TICK, max_sqrt_ratio, min_sqrt_ratio, sqrt_ratio_at_tick};

// Шаг-математика Elastic (math/swap_elastic_step.rs) — для котировки (81b) и тестов — доступна через этот модуль.
pub use super::swap_elastic_step::{
    Step, calc_final_price, calc_incremental_liquidity, calc_reach_amount, calc_returned_amount,
    compute_swap_step, estimate_incremental_liquidity,
};

/// MathConstants.sol: не больше стольких тиков за шаг (держит допущение ~5% движения цены в calcReachAmount).
const MAX_TICK_DISTANCE: i32 = 480;
const OVERFLOW: &str = "переполнение/недобор в математике Elastic";

/// Состояние пула Elastic для расчёта обмена.
pub struct ElasticState<'a> {
    pub sqrt_p: U256,
    pub current_tick: i32,
    pub base_l: u128,
    pub reinvest_l: u128,
    /// Комиссия пула в feeUnits (FEE_UNITS = 100000 = 100%).
    pub fee_units: u32,
    pub ticks: &'a TickMap,
}

/// Итог обмена.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Swapped {
    pub amount_in: U256,
    pub amount_out: U256,
    /// true — вся заданная сумма исполнена в прочитанных пределах карты тиков.
    pub complete: bool,
}

/// Обмен (Pool.swap). Принимает: состояние, is_token0 (сумма задана в token0), exact_in, сумму. Возвращает: Swapped;
/// Err — там, где контракт откатился бы или карта тиков не сходится.
pub fn swap(
    s: &ElasticState,
    is_token0: bool,
    exact_in: bool,
    amount: U256,
) -> Result<Swapped, &'static str> {
    // willUpTick (цена растёт) = exactInput != isToken0. Предел — min/max sqrtP (полный обмен).
    let will_up = exact_in != is_token0;
    let limit = if will_up {
        max_sqrt_ratio() - U256::from(1u8)
    } else {
        min_sqrt_ratio() + U256::from(1u8)
    };
    swap_to(s, is_token0, exact_in, amount, limit).map(|(r, _)| r)
}

/// Обмен до предела цены `limit` (как Pool.swap с limitSqrtP). Возвращает: итог и дошла ли цена до предела. Ошибки:
/// как swap. Решение 73 (глубина в полосе цены) — через limit.
pub fn swap_to(
    s: &ElasticState,
    is_token0: bool,
    exact_in: bool,
    amount: U256,
    limit: U256,
) -> Result<(Swapped, bool), &'static str> {
    let will_up = exact_in != is_token0;
    let fee = s.fee_units;
    let (mut sqrt, mut cur_tick) = (s.sqrt_p, s.current_tick);
    let (mut base_l, mut reinvest_l) = (U256::from(s.base_l), U256::from(s.reinvest_l));
    let mut specified = if exact_in {
        I256::try_from(amount).map_err(|_| OVERFLOW)?
    } else {
        rev_to_int(amount).ok_or(OVERFLOW)?
    };
    let (mut total_used, mut total_ret) = (U256::ZERO, U256::ZERO);
    let (lo, hi) = window_ticks(s.ticks);
    while !specified.is_zero() && sqrt != limit {
        // Следующий инициализированный тик в сторону обмена (Pool.sol: up — строго выше, down — ≤ текущего;
        // _getInitialSwapData/_updateLiquidityAndCrossTick). Нет такого в прочитанном окне — идём к краю окна.
        let init = next_init(s.ticks, cur_tick, will_up);
        let (bound, is_cross) = match init {
            Some(t) if t >= lo && t <= hi => (t, true),
            _ => (if will_up { hi } else { lo }, false),
        };
        // Ограничение шага MAX_TICK_DISTANCE (480 тиков) — так держится допущение ~5% движения цены в calcReachAmount;
        // reinvestL прирастает на КАЖДОМ шаге, поэтому точки дробления (480 и инициализированные тики) — как в контракте.
        let temp_next = if will_up {
            bound.min(cur_tick + MAX_TICK_DISTANCE)
        } else {
            bound.max(cur_tick - MAX_TICK_DISTANCE)
        }
        .clamp(MIN_TICK, MAX_TICK);
        let next_sqrt_full =
            sqrt_ratio_at_tick(temp_next).ok_or("цена тика вне диапазона TickMath")?;
        let target = if will_up == (next_sqrt_full > limit) {
            limit
        } else {
            next_sqrt_full
        };
        let liquidity = base_l.checked_add(reinvest_l).ok_or(OVERFLOW)?;
        let st = compute_swap_step(liquidity, sqrt, target, fee, specified, exact_in, is_token0)
            .ok_or("шаг Elastic не считается (переполнение)")?;
        sqrt = st.next_sqrt;
        specified = specified.checked_sub(st.used).ok_or(OVERFLOW)?;
        total_used = total_used.checked_add(abs(st.used)).ok_or(OVERFLOW)?;
        total_ret = total_ret.checked_add(abs(st.returned)).ok_or(OVERFLOW)?;
        reinvest_l = reinvest_l.checked_add(st.delta_l).ok_or(OVERFLOW)?;
        if sqrt != next_sqrt_full {
            // Цена не дошла до (временной) цели — сумма исчерпана или достигнут предел: обмен завершён.
            break;
        }
        cur_tick = if will_up { temp_next } else { temp_next - 1 };
        // Ограничение 480 не дало настоящего инициализированного тика — пересечения нет, идём дальше.
        if temp_next != bound {
            continue;
        }
        if !is_cross {
            // Дошли до края прочитанного окна — за ним инициализированные тики неизвестны: итог частичный.
            return Ok((partial(total_used, total_ret, exact_in), false));
        }
        // Пересечение инициализированного тика: baseL += liquidityNet (вверх) / −= (вниз) — PoolTicksState.
        let net = s
            .ticks
            .ticks
            .get(&bound)
            .ok_or("тик отмечен в окне, но его нет в карте тиков")?
            .net;
        let net = if will_up {
            net
        } else {
            net.checked_neg().ok_or(OVERFLOW)?
        };
        base_l = if net >= 0 {
            base_l.checked_add(U256::from(net.unsigned_abs()))
        } else {
            base_l.checked_sub(U256::from(net.unsigned_abs()))
        }
        .ok_or("baseL после пересечения тика вне диапазона (карта тиков не сходится)")?;
    }
    let complete = specified.is_zero();
    let out = finish(total_used, total_ret, exact_in, complete);
    Ok((out, sqrt == limit))
}

/// Следующий инициализированный тик карты в сторону обмена: вверх — строго больше `cur`; вниз — не больше `cur`
/// (Pool.sol _getInitialSwapData). None — в прочитанных тиках такого нет. Ошибок нет.
fn next_init(t: &TickMap, cur: i32, up: bool) -> Option<i32> {
    if up {
        t.ticks.range((cur + 1)..).next().map(|(k, _)| *k)
    } else {
        t.ticks.range(..=cur).next_back().map(|(k, _)| *k)
    }
}

/// Диапазон тиков, для которого прочитаны ВСЕ инициализированные тики (прочитанные слова битмапа): слово w покрывает
/// тики [w·256·spacing, (w·256+255)·spacing]. За ним математика не заходит (решение как у V3, swap_cl). Ошибок нет.
fn window_ticks(t: &TickMap) -> (i32, i32) {
    let sp = t.spacing;
    let lo = t.words.0 as i32 * 256 * sp;
    let hi = (t.words.1 as i32 * 256 + 255) * sp;
    (lo.clamp(MIN_TICK, MAX_TICK), hi.clamp(MIN_TICK, MAX_TICK))
}

/// Частичный итог (цена вышла за прочитанное окно тиков): complete = false.
fn partial(total_used: U256, total_ret: U256, exact_in: bool) -> Swapped {
    finish(total_used, total_ret, exact_in, false)
}

/// Сводит суммы в (вход, выход): при exactInput used — вход, returned — выход; при exactOutput наоборот.
fn finish(total_used: U256, total_ret: U256, exact_in: bool, complete: bool) -> Swapped {
    let (amount_in, amount_out) = if exact_in {
        (total_used, total_ret)
    } else {
        (total_ret, total_used)
    };
    Swapped {
        amount_in,
        amount_out,
        complete,
    }
}
