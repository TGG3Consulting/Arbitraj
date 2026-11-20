//! Файл: venues/bsc_dex/math/lb_fee.rs
//! Что делает: комиссия пула Liquidity Book V2.1 (решение 290) — точный порт PairParameterHelper (обновление опорных
//! значений по времени блока, накопитель волатильности по пройденным бинам, полная комиссия = база + изменчивая часть) и
//! FeeHelper (комиссия из суммы с комиссией и сверх суммы, предел MAX_FEE). Изменчивая часть меняется без событий пула —
//! по времени блока (updateReferences) — поэтому вид перечитывается каждым блоком (pools/per_block_lb.rs).
//! Решение 445: у пула v2.0 (LbBinData.v20) опорные значения и накопитель — по правилам v2.0 (math/lb20_fee.rs);
//! полная комиссия (total_fee) у обеих версий одна.
//! Кто вызывает: math/lb_swap.rs, math/lb20_fee.rs, math/lb20_swap.rs, quote/steps.rs (комиссия активного бина для
//! экрана), quote/breaks_bins.rs.
//! От чего зависит: core/types (LbBinData), alloy-primitives (U256).
//! Настройки: нет.
//!
//! Источник (решение 23): github.com/lfj-gg/joe-v2@v2.1.0 (8eeb42d7) = Sourcify exact_match реализаций пула BSC
//! 0xee5a9009…86df и 0x3e30fdae…2d8f (у второй отличия от v2.1.0 — только pragma и TreeMath.remove; tests/data/
//! lb_v21_2026-11-15/README.md). Строки — src/libraries/PairParameterHelper.sol: getBaseFee L225-L230, getVariableFee
//! L238-L249, getTotalFee L257-L261, setVolatilityReference L279-L283, setVolatilityAccumulator L291-L295,
//! updateVolatilityReference L371-L381, updateVolatilityAccumulator L389-L405, updateReferences L412-L421;
//! src/libraries/FeeHelper.sol: verifyFee L20-L23, getFeeAmountFrom L40-L50, getFeeAmount L58-L64;
//! src/libraries/Constants.sol L14 (PRECISION), L17 (MAX_FEE), L20 (BASIS_POINT_MAX). Выдержки —
//! tests/data/lb_v21_2026-11-15/source_excerpt.txt.

use alloy_primitives::U256;

use crate::core::types::LbBinData;

/// Constants.PRECISION = 1e18 (Constants.sol L14).
pub const PRECISION: u128 = 1_000_000_000_000_000_000;
/// Constants.MAX_FEE = 0.1e18 — 10 % (Constants.sol L17); комиссия больше — откат FeeHelper__FeeTooLarge (FeeHelper.sol L21).
const MAX_FEE: u128 = 100_000_000_000_000_000;
/// Constants.BASIS_POINT_MAX = 10_000 (Constants.sol L20).
const BASIS_POINT_MAX: u64 = 10_000;
/// Множитель базовой комиссии: baseFactor · binStep · 1e10 (PairParameterHelper.sol L228).
const BASE_FEE_SCALE: u128 = 10_000_000_000;
/// Делитель изменчивой части: (prod² · variableFeeControl + 99) / 100 (PairParameterHelper.sol L246).
const VARIABLE_FEE_DIV: u128 = 100;
/// Encoded.MASK_UINT20 — предел накопителя и опорной волатильности (setVolatilityReference / setVolatilityAccumulator
/// откатываются выше, PairParameterHelper.sol L280, L292).
const MASK_UINT20: u64 = 0xF_FFFF;

/// Изменчивые параметры комиссии в ходе обмена (то, что обмен меняет в своей копии параметров пула).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Volatility {
    /// volatilityAccumulator.
    pub acc: u64,
    /// volatilityReference.
    pub reference: u64,
    /// idReference.
    pub id_reference: u64,
}

/// updateReferences(block.timestamp) (L412-L421): прошло не меньше filterPeriod — опорный бин = активный, опорная
/// волатильность = накопитель · reductionFactor / 10000 (если прошло меньше decayPeriod) или 0. Принимает: состояние,
/// время блока обмена. Возвращает: изменчивые параметры; None — время блока раньше последнего обновления (вычитание
/// L413 откатилось бы) или опорная волатильность больше uint20 (L280).
pub fn references(d: &LbBinData, now: u64) -> Option<Volatility> {
    // Решение 445: пул v2.0 — FeeHelper v2.0 (lb20_fee.rs).
    if d.v20 {
        return super::lb20_fee::references(d, now);
    }
    let dt = now.checked_sub(d.time_of_last_update)?;
    let mut v = Volatility {
        acc: u64::from(d.volatility_accumulator),
        reference: u64::from(d.volatility_reference),
        id_reference: u64::from(d.id_reference),
    };
    if dt >= u64::from(d.filter_period) {
        v.id_reference = u64::from(d.active_id);
        v.reference = if dt < u64::from(d.decay_period) {
            v.acc * u64::from(d.reduction_factor) / BASIS_POINT_MAX
        } else {
            0
        };
        if v.reference > MASK_UINT20 {
            return None;
        }
    }
    Some(v)
}

/// updateVolatilityAccumulator(activeId) (L389-L405): накопитель = min(опорная + |id − опорный| · 10000, предел).
/// Принимает: изменчивые параметры, состояние (предел), бин. Возвращает: новые параметры; None — больше uint20 (L292).
pub fn accumulate(v: Volatility, d: &LbBinData, id: u32) -> Option<Volatility> {
    // Решение 445: пул v2.0 — предел uint24 без отката (lb20_fee.rs).
    if d.v20 {
        return Some(super::lb20_fee::accumulate(v, d, id));
    }
    let delta = v.id_reference.abs_diff(u64::from(id));
    let acc = (v.reference + delta * BASIS_POINT_MAX).min(u64::from(d.max_volatility_accumulator));
    if acc > MASK_UINT20 {
        return None;
    }
    Some(Volatility { acc, ..v })
}

/// getTotalFee(binStep) (L257-L261) = getBaseFee + getVariableFee, точность 1e18. Принимает: состояние, накопитель.
/// Возвращает: полную комиссию; None — больше uint128 (safe128). Ошибок нет.
pub fn total_fee(d: &LbBinData, acc: u64) -> Option<u128> {
    let step = U256::from(d.bin_step);
    let base = U256::from(d.base_factor) * step * U256::from(BASE_FEE_SCALE);
    let control = U256::from(d.variable_fee_control);
    let variable = if control.is_zero() {
        U256::ZERO
    } else {
        let prod = U256::from(acc) * step;
        (prod * prod * control + U256::from(VARIABLE_FEE_DIV - 1)) / U256::from(VARIABLE_FEE_DIV)
    };
    (base + variable).try_into().ok()
}

/// getFeeAmountFrom (L40-L50): комиссия из суммы с комиссией, вверх. None — комиссия больше MAX_FEE (откат). Ошибок нет.
pub fn fee_from(amount_with_fees: u128, fee: u128) -> Option<u128> {
    if fee > MAX_FEE {
        return None;
    }
    let r = (U256::from(amount_with_fees) * U256::from(fee) + U256::from(PRECISION - 1))
        / U256::from(PRECISION);
    Some(r.to::<u128>())
}

/// getFeeAmount (L58-L64): комиссия сверх суммы, вверх: (amount · fee + (1e18 − fee) − 1) / (1e18 − fee). None —
/// комиссия больше MAX_FEE (откат). Ошибок нет.
pub fn fee_on(amount: u128, fee: u128) -> Option<u128> {
    if fee > MAX_FEE {
        return None;
    }
    let den = U256::from(PRECISION - fee);
    let r = (U256::from(amount) * U256::from(fee) + den - U256::from(1u8)) / den;
    Some(r.to::<u128>())
}

/// Полная комиссия в активном бине на времени блока чтения (как у первого бина обмена) — для экрана. None — как откат.
pub fn active_fee(d: &LbBinData) -> Option<u128> {
    let v = accumulate(references(d, d.block_ts)?, d, d.active_id)?;
    total_fee(d, v.acc)
}
