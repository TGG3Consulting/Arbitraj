//! Файл: core/types/pool_lb.rs
//! Что делает: состояние пула «из бинов с изменчивой комиссией» (решение 290) — без названия площадки: активный бин, шаг
//! бина, какой токен пула — X, постоянные параметры комиссии (база, окна фильтра и затухания, коэффициент снижения,
//! вес изменчивой части, доля протокола, предел накопителя волатильности), изменчивые (накопитель, опорная
//! волатильность, опорный бин, время последнего обновления) и время блока чтения. Изменчивая часть комиссии зависит от
//! времени блока и от числа пройденных бинов — её считает математика адаптера. Резервы бинов в стык не входят (живут в
//! адаптере, как у Bin). Вынесено из pool.rs (предел размера файла).
//! Кто вызывает: core/types/pool.rs (PoolState::LbBin), адаптер DEX (pools/load_lb.rs создаёт, math/lb_swap.rs считает).
//! От чего зависит: serde.
//! Настройки: нет.

use serde::{Deserialize, Serialize};

/// Состояние пула из бинов с изменчивой комиссией на блоке — боксируется в `PoolState::LbBin`. Числа — как отдаёт пул
/// (getActiveId, getBinStep, getStaticFeeParameters, getVariableFeeParameters).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LbBinData {
    /// Активный бин (uint24).
    pub active_id: u32,
    /// Шаг бина (uint16, сотые доли процента).
    pub bin_step: u32,
    /// Токен X пула — token0 записи (иначе X — token1). Порядок X/Y задаёт создатель пула, не адреса.
    pub x_is_token0: bool,
    /// baseFactor (uint16).
    pub base_factor: u32,
    /// filterPeriod, секунды (uint12).
    pub filter_period: u32,
    /// decayPeriod, секунды (uint12).
    pub decay_period: u32,
    /// reductionFactor, базисные пункты (uint14).
    pub reduction_factor: u32,
    /// variableFeeControl (uint24).
    pub variable_fee_control: u32,
    /// protocolShare, базисные пункты (uint14).
    pub protocol_share: u32,
    /// maxVolatilityAccumulator (uint20).
    pub max_volatility_accumulator: u32,
    /// volatilityAccumulator (uint20).
    pub volatility_accumulator: u32,
    /// volatilityReference (uint20).
    pub volatility_reference: u32,
    /// idReference (uint24).
    pub id_reference: u32,
    /// timeOfLastUpdate, секунды Unix (uint40).
    pub time_of_last_update: u64,
    /// Время блока, на котором прочитано состояние (секунды Unix) — по нему котировка считает изменчивую комиссию.
    pub block_ts: u64,
}
