//! Файл: venues/bsc_dex/pools/aero_first_fee.rs
//! Что делает: решение 470 (T470-01) — порт «первой комиссии блока» крючка DynamicSwapFeeHook (Aero Slipstream V3) без
//! обращений к узлу: _getFirstSwapFee и _getDynamicFee (TWAP-тик по observe пула) — для комиссии, которую возьмёт
//! первый обмен СЛЕДУЮЩЕГО блока (время блока = время головы + δ). Данные читает pools/aero_cl_head.rs.
//! Кто вызывает: pools/aero_cl_head.rs.
//! От чего зависит: alloy_primitives (U256).
//! Настройки: нет.
//!
//! Источник — Dromos-Labs/metadex-public 1.1.0 (79c8d5b6) V3/src/hooks/dynamic/DynamicSwapFeeHook.sol:
//! - константы ZERO_FEE_INDICATOR = type(uint24).max (L47), SCALING_PRECISION = 1e6 (L53);
//! - _getFee L524-L544: blockFee блока = 0 (первый обмен блока) — _getFirstSwapFee; иначе — blockFee (со скидкой
//!   DiscountRegistry, скидка только уменьшает, L673-L697) или initialFee первой транзакции;
//! - _getFirstSwapFee L569-L619: scalingFactor пула 0 — defaultScalingFactor и defaultFeeCap крючка; baseFee =
//!   ZERO_FEE_INDICATOR — (tickSpacingToFee, tickSpacingToFee); baseFee 0 — tickSpacingToFee; feeToStore =
//!   min(база + динамическая, feeCap); initialFeeEnabled: initialFee 0 — база, ZERO_FEE_INDICATOR — tickSpacingToFee,
//!   иначе initialFee (без предела); иначе feeToUse = скидка(feeToStore) ≤ feeToStore;
//! - _getDynamicFee L626-L666: cardinality 0 — 0; самое старое наблюдение (index + 1) % cardinality, не
//!   инициализировано — наблюдение 0; secondsAgo > время − его время — 0; TWAP-тик = Δcum / secondsAgo (деление
//!   Solidity — к нулю) и −1 при отрицательной Δcum с остатком; observe откатился — 0; |тик − TWAP| · K / 1e6.
//!   Арифметика — uint24 · uint256 (L662-L665): |Δтик| < 2^24, K ≤ 1e18 — без переполнения.
//!   Время в проверке «secondsAgo > время − старое» — uint32 (L640): время блока − время наблюдения, переполнения нет —
//!   наблюдение не новее блока.
//!
//! Δcum на время следующего блока T' = T + δ (T — время головы): cum(T') = cum(T) + тик·δ (Oracle.transform —
//! metadex-slipstream-public cf1ae46f contracts/core/libraries/Oracle.sol: обменов между головой и нашим нет, тик тот же),
//! cum(T' − S) = observe([S − δ]) на голове (та же цель времени, тот же откат «OLD», если цель старше старого
//! наблюдения — у крючка это try/catch → 0). Поэтому на каждое δ = 0…N — свой вызов observe([S − δ, 0]) на голове:
//! откат одного δ не обнуляет другие.

use alloy_primitives::U256;

/// ZERO_FEE_INDICATOR крючка (L47) — type(uint24).max.
pub const ZERO_FEE_INDICATOR: u32 = 0x00ff_ffff;
/// SCALING_PRECISION (L53).
const SCALING_PRECISION: u64 = 1_000_000;

/// DynamicFeeConfig пула (IDynamicSwapFeeHook.sol L21-L27; геттер dynamicFeeConfig(pool) L232-L235).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Config {
    pub base_fee: u32,
    pub fee_cap: u32,
    pub scaling: u64,
    pub initial_enabled: bool,
    pub initial_fee: u32,
}

/// Общие параметры крючка: defaultScalingFactor, defaultFeeCap (uint256), secondsAgo (uint32).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Defaults {
    pub scaling: U256,
    pub fee_cap: U256,
    pub seconds_ago: u32,
}

/// TWAP-тик (L652-L658). Принимает: Δcum = cum(новое) − cum(старое), окно секунд (> 0). Возвращает: тик. Ошибок нет.
pub fn tw_avg_tick(delta: i64, secs: u32) -> i64 {
    let s = i64::from(secs);
    let mut t = delta / s;
    if delta < 0 && delta % s != 0 {
        t -= 1;
    }
    t
}

/// Динамическая часть (L662-L665): |тик − TWAP| · K / 1e6. Принимает: текущий тик, TWAP-тик, K. Ошибок нет.
pub fn dynamic_fee(tick: i32, tw: i64, scaling: U256) -> U256 {
    // int24 _tickDelta = _currentTick − _twAvgTick: TWAP-тик — int24(…) (L652), разность int24.
    let abs = (i64::from(tick) - tw).unsigned_abs();
    U256::from(abs).saturating_mul(scaling) / U256::from(SCALING_PRECISION)
}

/// Первая комиссия блока (L569-L619) без скидки. Принимает: конфиг пула, общие параметры, tickSpacingToFee пула,
/// динамическую часть (вызывается только при base ≠ ZERO_FEE_INDICATOR — как в исходнике; при ZERO её не нужно).
/// Возвращает: (feeToUse без скидки, feeToStore). Ошибок нет.
pub fn first(cfg: &Config, d: &Defaults, ts_fee: u32, dynamic: U256) -> (U256, U256) {
    // L582-L584: scalingFactor пула 0 — defaultScalingFactor И defaultFeeCap (K — scaling_of, здесь только предел).
    let cap = if cfg.scaling == 0 {
        d.fee_cap
    } else {
        U256::from(cfg.fee_cap)
    };
    if cfg.base_fee == ZERO_FEE_INDICATOR {
        let t = U256::from(ts_fee);
        return (t, t);
    }
    let base = U256::from(if cfg.base_fee == 0 {
        ts_fee
    } else {
        cfg.base_fee
    });
    let store = base.saturating_add(dynamic).min(cap);
    let used = if cfg.initial_enabled {
        match cfg.initial_fee {
            0 => base,
            ZERO_FEE_INDICATOR => U256::from(ts_fee),
            x => U256::from(x),
        }
    } else {
        // _applyDiscount(feeToStore) ≤ feeToStore (скидка только уменьшает) — без скидки.
        store
    };
    (used, store)
}

/// K первой комиссии (L582-L584): scalingFactor пула, при нуле — defaultScalingFactor. Ошибок нет.
pub fn scaling_of(cfg: &Config, d: &Defaults) -> U256 {
    if cfg.scaling == 0 {
        d.scaling
    } else {
        U256::from(cfg.scaling)
    }
}

/// Наблюдение оракула пула — время и признак инициализации (Oracle.Observation, нужные поля).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Obs {
    pub time: u32,
    pub initialized: bool,
}

/// Данные пула для первой комиссии на голове.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// Время головы T (секунды).
    pub time: u64,
    pub tick: i32,
    pub cardinality: u16,
    /// Наблюдение (index + 1) % cardinality и наблюдение 0.
    pub oldest: Obs,
    pub zero: Obs,
    /// По δ = 0…N — ответ observe([S − δ, 0]) на голове: (cum(T − S + δ), cum(T)); None — откат (у крючка — 0).
    pub cums: Vec<Option<(i64, i64)>>,
}

/// Динамическая часть на время T + δ (L626-L666 при T' = T + δ). Принимает: данные головы, общие параметры, K, δ.
/// Возвращает: динамическую часть. Ошибок нет: недостающие данные — как ветки исходника (0).
pub fn dynamic_at(h: &Head, d: &Defaults, scaling: U256, delta: u32) -> U256 {
    if h.cardinality == 0 || d.seconds_ago == 0 {
        // secondsAgo = 0: деление на 0 в observe-ветке откатило бы крючок; setSecondsAgo запрещает 0 (MIN_SECONDS_AGO).
        return U256::ZERO;
    }
    let oldest = if h.oldest.initialized { h.oldest.time } else { h.zero.time };
    let now = h.time + u64::from(delta);
    if u64::from(d.seconds_ago) > now.saturating_sub(u64::from(oldest)) {
        return U256::ZERO;
    }
    let Some(Some((old, new))) = h.cums.get(delta as usize).copied() else {
        return U256::ZERO;
    };
    let cum_new = new + i64::from(h.tick) * i64::from(delta);
    dynamic_fee(h.tick, tw_avg_tick(cum_new - old, d.seconds_ago), scaling)
}

/// Худшая (наибольшая) первая комиссия блока за δ = 0…ahead: max(feeToUse без скидки, feeToStore) — кто бы ни был
/// первым в блоке (мы — feeToUse; чужой обмен раньше — blockFee = feeToStore). Принимает: конфиг, общие параметры,
/// tickSpacingToFee, данные головы, ahead. Возвращает: комиссию в пипсах (1e6), не выше u32::MAX. Ошибок нет.
pub fn worst(cfg: &Config, d: &Defaults, ts_fee: u32, h: &Head, ahead: u32) -> u32 {
    let k = scaling_of(cfg, d);
    let mut out = U256::ZERO;
    for delta in 0..=ahead.min(d.seconds_ago) {
        let (used, store) = first(cfg, d, ts_fee, dynamic_at(h, d, k, delta));
        out = out.max(used).max(store);
    }
    out.min(U256::from(u32::MAX)).to::<u32>()
}
