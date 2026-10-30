//! Файл: venues/bsc_dex/math/swap_elastic.rs
//! Что делает: обмен в пуле KyberSwap Elastic (концентрированная ликвидность с reinvestment liquidity, решение 81) на
//! локальной карте тиков — порт SwapMath.computeSwapStep и цикла Pool.swap из официального контракта ks-elastic-sc.
//! Отличие от Uniswap V3 (math/swap_cl.rs): ликвидность шага L = baseL + reinvestL; комиссия шага не идёт в отдельный
//! feeGrowth, а приращивает reinvestL (deltaL); формулы шага свои (calcReachAmount / estimate+calcIncrementalLiquidity /
//! calcFinalPrice / calcReturnedAmount, через BPS/TWO_FEE_UNITS и квадратное уравнение ΔL); шаг ограничен
//! MAX_TICK_DISTANCE (480 тиков — так держится допущение ~5% движения цены в calcReachAmount); при пересечении
//! инициализированного тика baseL меняется на liquidityNet, reinvestL переживает тики. TickMath (getSqrtRatioAtTick,
//! MIN/MAX_SQRT_RATIO) у Elastic идентичен Uniswap V3 — переиспользуем math/tick_math.rs.
//! ЭТОТ модуль — только математика (без сети): чтение состояния/тиков, события, исполнение — фазы 80.81b/81c.
//! Кто вызывает: quote/pool_quote.rs (через 81b); тесты.
//! От чего зависит: math/full_math.rs (mulDivFloor=mul_div, mulDivCeiling=mul_div_up), math/tick_math.rs,
//! pools/ticks.rs (TickMap). Свой поиск следующего тика (next_init), НЕ swap_cl::next_tick: у Elastic reinvestL
//! прирастает на каждом шаге, поэтому точки дробления обязаны быть ровно контрактными (инициализированные тики и
//! MAX_TICK_DISTANCE), а не границы слов битмапа — иначе ΔL-компаундинг разойдётся с QuoterV2.
//! Настройки: нет (комиссия пула feeUnits передаётся вызывающим — читается с контракта в 81b).
//!
//! Источники (официальные, решение 23): KyberNetwork/ks-elastic-sc (HEAD 4ab08c0):
//! - SwapMath.sol computeSwapStep / calcReachAmount / estimateIncrementalLiquidity / calcIncrementalLiquidity /
//!   calcFinalPrice / calcReturnedAmount — contracts/libraries/SwapMath.sol;
//! - QuadMath.getSmallerRootOfQuadEqn — contracts/libraries/QuadMath.sol;
//! - MathConstants (FEE_UNITS=100000, TWO_FEE_UNITS=200000, MAX_TICK_DISTANCE=480, RES_96=96) — libraries/MathConstants.sol;
//! - цикл swap, _updateLiquidityAndCrossTick (liquidityNet к baseL: +net вверх, −net вниз) — contracts/Pool.sol,
//!   contracts/PoolTicksState.sol;
//! - формулы также на docs.kyberswap.com (Core Libraries / SwapMath).

use alloy_primitives::{I256, U256};

use super::super::pools::ticks::TickMap;
use super::full_math::{mul_div, mul_div_up};
use super::tick_math::{MAX_TICK, MIN_TICK, max_sqrt_ratio, min_sqrt_ratio, sqrt_ratio_at_tick};

/// MathConstants.sol.
const FEE_UNITS: u32 = 100_000;
const TWO_FEE_UNITS: u32 = 200_000;
const MAX_TICK_DISTANCE: i32 = 480;
const OVERFLOW: &str = "переполнение/недобор в математике Elastic";

/// 2^96.
fn q96() -> U256 {
    U256::from(1u8) << 96
}

/// Квадратный корень вниз (QuadMath.sqrt — вавилонский метод, floor). Ошибок нет.
fn isqrt(y: U256) -> U256 {
    if y > U256::from(3u8) {
        let mut z = y;
        let mut x = y / U256::from(2u8) + U256::from(1u8);
        while x < z {
            z = x;
            x = (y / x + x) / U256::from(2u8);
        }
        z
    } else if y.is_zero() {
        U256::ZERO
    } else {
        U256::from(1u8)
    }
}

/// QuadMath.getSmallerRootOfQuadEqn: (b − sqrt(b² − a·c)) / a. None — b² < a·c (корня нет) или a = 0.
fn smaller_root(a: U256, b: U256, c: U256) -> Option<U256> {
    let disc = b.checked_mul(b)?.checked_sub(a.checked_mul(c)?)?;
    b.checked_sub(isqrt(disc))?.checked_div(a)
}

/// |I256| как U256. Ошибок нет.
fn abs(x: I256) -> U256 {
    x.unsigned_abs()
}

/// −U256 как I256 (SafeCast.revToInt256). None — не помещается в int256.
fn rev_to_int(y: U256) -> Option<I256> {
    I256::ZERO.checked_sub(I256::try_from(y).ok()?)
}

/// calcReachAmount (SwapMath.sol): сумма (с учётом комиссии) чтобы дойти от currentSqrtP до targetSqrtP.
/// exactInput — положительна, exactOutput — отрицательна. None — переполнение/недобор знаменателя (при валидных
/// входах и |Δцены| ≤ ~5% не наступает). Округление вниз (floor) — как в контракте.
pub fn calc_reach_amount(
    liquidity: U256,
    current: U256,
    target: U256,
    fee: u32,
    exact_in: bool,
    is_token0: bool,
) -> Option<I256> {
    let (two_fee, fee) = (U256::from(TWO_FEE_UNITS), U256::from(fee));
    let abs_diff = if current >= target {
        current - target
    } else {
        target - current
    };
    if exact_in {
        if is_token0 {
            let denom = two_fee
                .checked_mul(target)?
                .checked_sub(fee.checked_mul(current)?)?;
            let num = mul_div(liquidity, two_fee.checked_mul(abs_diff)?, denom)?;
            I256::try_from(mul_div(num, q96(), current)?).ok()
        } else {
            let denom = two_fee
                .checked_mul(current)?
                .checked_sub(fee.checked_mul(target)?)?;
            let num = mul_div(liquidity, two_fee.checked_mul(abs_diff)?, denom)?;
            I256::try_from(mul_div(num, current, q96())?).ok()
        }
    } else if is_token0 {
        let denom = two_fee
            .checked_mul(current)?
            .checked_sub(fee.checked_mul(target)?)?;
        let num = denom.checked_sub(fee.checked_mul(current)?)?;
        let num = mul_div(liquidity << 96, num, denom)?;
        rev_to_int(mul_div(num, abs_diff, current)?.checked_div(target)?)
    } else {
        let denom = two_fee
            .checked_mul(target)?
            .checked_sub(fee.checked_mul(current)?)?;
        let num = denom.checked_sub(fee.checked_mul(target)?)?;
        let num = mul_div(liquidity, num, denom)?;
        rev_to_int(mul_div(num, abs_diff, q96())?)
    }
}

/// estimateIncrementalLiquidity (SwapMath.sol): ΔL (комиссия шага → reinvestL) для ФИНАЛЬНОГО шага (тик не пересекаем).
/// exactOutput — меньший корень квадратного уравнения a·x²−2b·x+c=0. None — переполнение/нет корня.
pub fn estimate_incremental_liquidity(
    abs_delta: U256,
    liquidity: U256,
    current: U256,
    fee: u32,
    exact_in: bool,
    is_token0: bool,
) -> Option<U256> {
    let (two_fee, fee_units, fee) = (
        U256::from(TWO_FEE_UNITS),
        U256::from(FEE_UNITS),
        U256::from(fee),
    );
    if exact_in {
        if is_token0 {
            mul_div(current, abs_delta.checked_mul(fee)?, two_fee << 96)
        } else {
            mul_div(
                q96(),
                abs_delta.checked_mul(fee)?,
                two_fee.checked_mul(current)?,
            )
        }
    } else {
        let a = fee;
        let mut b = fee_units.checked_sub(fee)?.checked_mul(liquidity)?;
        let mut c = fee.checked_mul(liquidity)?.checked_mul(abs_delta)?;
        if is_token0 {
            b = b.checked_sub(mul_div(fee_units.checked_mul(abs_delta)?, current, q96())?)?;
            c = mul_div(c, current, q96())?;
        } else {
            b = b.checked_sub(mul_div(fee_units.checked_mul(abs_delta)?, q96(), current)?)?;
            c = mul_div(c, q96(), current)?;
        }
        smaller_root(a, b, c)
    }
}

/// calcIncrementalLiquidity (SwapMath.sol): ΔL для промежуточного шага (тик пересекаем). None — переполнение.
pub fn calc_incremental_liquidity(
    abs_delta: U256,
    liquidity: U256,
    current: U256,
    next: U256,
    exact_in: bool,
    is_token0: bool,
) -> Option<U256> {
    let tmp3 = if is_token0 {
        let tmp1 = mul_div(liquidity, q96(), current)?;
        let tmp2 = if exact_in {
            tmp1.checked_add(abs_delta)?
        } else {
            tmp1.checked_sub(abs_delta)?
        };
        mul_div(next, tmp2, q96())?
    } else {
        let tmp1 = mul_div(liquidity, current, q96())?;
        let tmp2 = if exact_in {
            tmp1.checked_add(abs_delta)?
        } else {
            tmp1.checked_sub(abs_delta)?
        };
        mul_div(tmp2, q96(), next)?
    };
    Some(if tmp3 > liquidity {
        tmp3 - liquidity
    } else {
        U256::ZERO
    })
}

/// calcFinalPrice (SwapMath.sol): новая sqrtP для финального шага. None — переполнение/недобор.
pub fn calc_final_price(
    abs_delta: U256,
    liquidity: U256,
    delta_l: U256,
    current: U256,
    exact_in: bool,
    is_token0: bool,
) -> Option<U256> {
    if is_token0 {
        let tmp = mul_div(abs_delta, current, q96())?;
        if exact_in {
            mul_div_up(
                liquidity.checked_add(delta_l)?,
                current,
                liquidity.checked_add(tmp)?,
            )
        } else {
            mul_div(
                liquidity.checked_add(delta_l)?,
                current,
                liquidity.checked_sub(tmp)?,
            )
        }
    } else {
        let tmp = mul_div(abs_delta, q96(), current)?;
        if exact_in {
            mul_div(
                liquidity.checked_add(tmp)?,
                current,
                liquidity.checked_add(delta_l)?,
            )
        } else {
            mul_div_up(
                liquidity.checked_sub(tmp)?,
                current,
                liquidity.checked_add(delta_l)?,
            )
        }
    }
}

/// calcReturnedAmount (SwapMath.sol): выход (exactInput, <0) или вход (exactOutput, >0). Округления по контракту:
/// выход вниз, вход вверх. None — переполнение.
pub fn calc_returned_amount(
    liquidity: U256,
    current: U256,
    next: U256,
    delta_l: U256,
    exact_in: bool,
    is_token0: bool,
) -> Option<I256> {
    let mut returned: I256 = if is_token0 {
        if exact_in {
            I256::try_from(mul_div_up(delta_l, next, q96())?)
                .ok()?
                .checked_add(rev_to_int(mul_div(
                    liquidity,
                    current.checked_sub(next)?,
                    q96(),
                )?)?)?
        } else {
            I256::try_from(mul_div_up(delta_l, next, q96())?)
                .ok()?
                .checked_add(
                    I256::try_from(mul_div_up(liquidity, next.checked_sub(current)?, q96())?)
                        .ok()?,
                )?
        }
    } else {
        I256::try_from(mul_div_up(liquidity.checked_add(delta_l)?, q96(), next)?)
            .ok()?
            .checked_add(rev_to_int(mul_div(liquidity, q96(), current)?)?)?
    };
    if exact_in && returned == I256::ONE {
        returned = I256::ZERO;
    }
    Some(returned)
}

/// Итог одного шага обмена Elastic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// Использованная сумма (+ exactInput вход / − exactOutput выход).
    pub used: I256,
    /// Встречная сумма (− выход при exactInput / + вход при exactOutput).
    pub returned: I256,
    /// Приращение reinvestL (комиссия шага).
    pub delta_l: U256,
    /// Новая sqrtP после шага.
    pub next_sqrt: U256,
}

/// SwapMath.computeSwapStep: один шаг обмена к targetSqrtP (не превышая его). Принимает: ликвидность baseL+reinvestL,
/// текущую/целевую sqrtP, комиссию feeUnits, остаток суммы (знак: + exactInput, − exactOutput), флаги exactInput и
/// token0. Возвращает: шаг. None — переполнение/недобор (при |Δцены| ≤ ~5% и валидных входах не наступает).
pub fn compute_swap_step(
    liquidity: U256,
    current: U256,
    target: U256,
    fee: u32,
    specified: I256,
    exact_in: bool,
    is_token0: bool,
) -> Option<Step> {
    if current == target {
        return Some(Step {
            used: I256::ZERO,
            returned: I256::ZERO,
            delta_l: U256::ZERO,
            next_sqrt: current,
        });
    }
    let mut used = calc_reach_amount(liquidity, current, target, fee, exact_in, is_token0)?;
    let mut next_sqrt = U256::ZERO;
    if (exact_in && used > specified) || (!exact_in && used <= specified) {
        used = specified;
    } else {
        next_sqrt = target;
    }
    let abs_delta = abs(used);
    let delta_l = if next_sqrt.is_zero() {
        let d = estimate_incremental_liquidity(
            abs_delta, liquidity, current, fee, exact_in, is_token0,
        )?;
        next_sqrt = calc_final_price(abs_delta, liquidity, d, current, exact_in, is_token0)?;
        d
    } else {
        calc_incremental_liquidity(
            abs_delta, liquidity, current, next_sqrt, exact_in, is_token0,
        )?
    };
    let returned =
        calc_returned_amount(liquidity, current, next_sqrt, delta_l, exact_in, is_token0)?;
    Some(Step {
        used,
        returned,
        delta_l,
        next_sqrt,
    })
}

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
        let step = compute_swap_step(liquidity, sqrt, target, fee, specified, exact_in, is_token0)
            .ok_or("шаг Elastic не считается (переполнение)")?;
        sqrt = step.next_sqrt;
        specified = specified.checked_sub(step.used).ok_or(OVERFLOW)?;
        total_used = total_used.checked_add(abs(step.used)).ok_or(OVERFLOW)?;
        total_ret = total_ret.checked_add(abs(step.returned)).ok_or(OVERFLOW)?;
        reinvest_l = reinvest_l.checked_add(step.delta_l).ok_or(OVERFLOW)?;
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
