//! Файл: venues/bsc_dex/math/lb20_fee.rs
//! Что делает: решение 445 — комиссия пула Liquidity Book v2.0 (LFJ V2 / Trader Joe V2, Arbitrum) — точный порт
//! FeeHelper v2.0: обновление опорных значений по времени блока (updateVariableFeeParameters), накопитель волатильности
//! по бину (updateVolatilityAccumulated), комиссия сверх суммы (getFeeAmount) и из суммы с комиссией (getFeeAmountFrom).
//! Полная комиссия (база + изменчивая часть) считается так же, как у v2.1 (lb_fee.rs total_fee: формулы getBaseFee,
//! getVariableFee, getTotalFee совпадают построчно). Отличия от v2.1 (lb_fee.rs) — только эти:
//!
//! - опорные значения обновляются и при нулевом времени последнего обновления (`|| _fp.time == 0`, L60);
//! - пределы накопителя и опорной волатильности — uint24 (у v2.1 — uint20 с откатом): опорная = uint24(…) без
//!   проверки (L65-L67, unchecked), накопитель = min(…, maxVolatilityAccumulated) (L85-L87);
//! - предела MAX_FEE у пула нет (у v2.1 — откат FeeHelper__FeeTooLarge): комиссия больше 1e18 — откат вычитания
//!   PRECISION − fee (L130, solc 0.8.10 checked), ровно 1e18 — деление на ноль; сумма комиссии — safe128
//!   (getFeeAmountDistribution L154). Фабрика v2.0 сама держит базу + наибольшую изменчивую часть ≤ 10 %
//!   (LBFactory.sol L562-L572).
//!
//! Кто вызывает: math/lb_fee.rs (references, accumulate — ветка v20), math/lb20_swap.rs.
//! От чего зависит: core/types (LbBinData), math/lb_fee.rs (Volatility, PRECISION), alloy-primitives (U256).
//! Настройки: нет.
//!
//! Источник (решение 23): github.com/lfj-gg/joe-v2, тег v2.0.0 (коммит 07f10212e2de6abc97932240c27388c14868582c) —
//! src/libraries/FeeHelper.sol: FeeParameters L29-L44, updateVariableFeeParameters L57-L77, updateVolatilityAccumulated
//! L82-L88, getBaseFee L93-L97, getVariableFee L102-L112, getFeeAmountFrom L119-L121, getFeeAmount L128-L132,
//! getFeeAmountDistribution L149-L159, getTotalFee L164-L168; src/libraries/Constants.sol L12-L13 (PRECISION,
//! BASIS_POINT_MAX); src/LBFactory.sol L540-L589 (_getPackedFeeParameters: проверки фабрики).

use alloy_primitives::U256;

use crate::core::types::LbBinData;

use super::lb_fee::{PRECISION, Volatility};

/// Constants.BASIS_POINT_MAX = 10_000 (Constants.sol L13).
const BASIS_POINT_MAX: u64 = 10_000;
/// Маска uint24 — приведение uint24(…) опорной волатильности (FeeHelper.sol L65-L67, unchecked).
const MASK_UINT24: u64 = 0xFF_FFFF;

/// updateVariableFeeParameters(activeId) без шага накопителя (L57-L74): прошло не меньше filterPeriod или время
/// последнего обновления 0 — опорный бин = активный, опорная волатильность = uint24(reductionFactor · накопитель /
/// 10000) при прошедшем меньше decayPeriod, иначе 0. Принимает: состояние, время блока обмена. Возвращает: изменчивые
/// параметры; None — время блока раньше последнего обновления (вычитание L58 откатилось бы). Ошибок нет.
pub fn references(d: &LbBinData, now: u64) -> Option<Volatility> {
    let dt = now.checked_sub(d.time_of_last_update)?;
    let mut v = Volatility {
        acc: u64::from(d.volatility_accumulator),
        reference: u64::from(d.volatility_reference),
        id_reference: u64::from(d.id_reference),
    };
    if dt >= u64::from(d.filter_period) || d.time_of_last_update == 0 {
        v.id_reference = u64::from(d.active_id);
        v.reference = if dt < u64::from(d.decay_period) {
            (u64::from(d.reduction_factor) * v.acc / BASIS_POINT_MAX) & MASK_UINT24
        } else {
            0
        };
    }
    Some(v)
}

/// updateVolatilityAccumulated(activeId) (L82-L88): накопитель = |id − опорный| · 10000 + опорная, не больше
/// maxVolatilityAccumulated. Принимает: изменчивые параметры, состояние (предел), бин. Возвращает: новые параметры.
/// Ошибок нет (сумма в uint256 не переполняется, предел — uint24).
pub fn accumulate(v: Volatility, d: &LbBinData, id: u32) -> Volatility {
    let delta = v.id_reference.abs_diff(u64::from(id));
    let acc = (delta * BASIS_POINT_MAX + v.reference).min(u64::from(d.max_volatility_accumulator));
    Volatility { acc, ..v }
}

/// getFeeAmount (L128-L132): комиссия сверх суммы, вверх: (amount · fee + (1e18 − fee) − 1) / (1e18 − fee), сумма —
/// uint256. Принимает: сумму, полную комиссию. Возвращает: комиссию; None — откат (fee ≥ 1e18: вычитание или деление на
/// ноль; сумма комиссии больше uint128 — safe128 L154; произведение больше uint256). Ошибок нет.
pub fn fee_on(amount: U256, fee: u128) -> Option<U256> {
    let den = PRECISION.checked_sub(fee).filter(|x| *x != 0)?;
    let den = U256::from(den);
    let num = amount.checked_mul(U256::from(fee))?;
    let r = num.checked_add(den - U256::from(1u8))? / den;
    (r <= U256::from(u128::MAX)).then_some(r)
}

/// getFeeAmountFrom (L119-L121): комиссия из суммы с комиссией, вверх: (amount · fee + 1e18 − 1) / 1e18. Принимает:
/// сумму с комиссией, полную комиссию. Возвращает: комиссию; None — больше uint128 (safe128 L154). Ошибок нет.
pub fn fee_from(amount_with_fees: u128, fee: u128) -> Option<u128> {
    let r = (U256::from(amount_with_fees) * U256::from(fee) + U256::from(PRECISION - 1))
        / U256::from(PRECISION);
    r.try_into().ok()
}
