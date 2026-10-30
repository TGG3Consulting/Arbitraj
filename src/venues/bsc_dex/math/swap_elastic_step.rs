//! Файл: venues/bsc_dex/math/swap_elastic_step.rs
//! Что делает: ШАГ-математика обмена KyberSwap Elastic (решение 81) — порт SwapMath.computeSwapStep и его частей из
//! официального контракта ks-elastic-sc. Один шаг к целевой цене: ликвидность шага L = baseL + reinvestL; комиссия
//! шага не идёт в отдельный feeGrowth, а приращивает reinvestL (deltaL); формулы свои (calcReachAmount /
//! estimate+calcIncrementalLiquidity / calcFinalPrice / calcReturnedAmount, через BPS/TWO_FEE_UNITS и квадратное
//! уравнение ΔL). Чистая математика на целых числах, без сети и без карты тиков — цикл по тикам в math/swap_elastic.rs.
//! Кто вызывает: math/swap_elastic.rs (цикл Pool.swap); quote-путь и тесты — через переэкспорт в swap_elastic.
//! От чего зависит: math/full_math.rs (mulDivFloor=mul_div, mulDivCeiling=mul_div_up). TickMath Elastic = Uniswap V3
//! (math/tick_math.rs) используется в цикле, не здесь.
//! Настройки: нет (комиссия пула feeUnits передаётся вызывающим).
//!
//! Источники (официальные, решение 23): KyberNetwork/ks-elastic-sc (HEAD 4ab08c0):
//! - computeSwapStep / calcReachAmount / estimateIncrementalLiquidity / calcIncrementalLiquidity / calcFinalPrice /
//!   calcReturnedAmount — contracts/libraries/SwapMath.sol;
//! - QuadMath.getSmallerRootOfQuadEqn (меньший корень a·x²−2b·x+c=0) — contracts/libraries/QuadMath.sol;
//! - MathConstants (FEE_UNITS=100000, TWO_FEE_UNITS=200000, RES_96=96) — contracts/libraries/MathConstants.sol;
//! - SafeCast.revToInt256 (−x) — contracts/libraries/SafeCast.sol;
//! - формулы также на docs.kyberswap.com (Core Libraries / SwapMath).

use alloy_primitives::{I256, U256};

use super::full_math::{mul_div, mul_div_up};

/// MathConstants.sol.
const FEE_UNITS: u32 = 100_000;
const TWO_FEE_UNITS: u32 = 200_000;

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

/// |I256| как U256. Ошибок нет. (Общее с циклом swap_elastic.rs.)
pub(crate) fn abs(x: I256) -> U256 {
    x.unsigned_abs()
}

/// −U256 как I256 (SafeCast.revToInt256). None — не помещается в int256. (Общее с циклом swap_elastic.rs.)
pub(crate) fn rev_to_int(y: U256) -> Option<I256> {
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
