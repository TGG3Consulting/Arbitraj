//! Файл: core/types/pool_lb.rs
//! Что делает: состояние пула «из бинов с изменчивой комиссией» (решение 290) — без названия площадки: активный бин, шаг
//! бина, какой токен пула — X, постоянные параметры комиссии (база, окна фильтра и затухания, коэффициент снижения,
//! вес изменчивой части, доля протокола, предел накопителя волатильности), изменчивые (накопитель, опорная
//! волатильность, опорный бин, время последнего обновления) и время блока чтения; у вида с крючками (решение 446) —
//! слово крючков и причина исключения обмена. Изменчивая часть комиссии зависит от
//! времени блока и от числа пройденных бинов — её считает математика адаптера. Резервы бинов в стык не входят (живут в
//! адаптере, как у Bin). Вынесено из pool.rs (предел размера файла).
//! Решение 445: тот же вид у Liquidity Book v2.0 (LFJ V2, Arbitrum) — признак v20: правила v2.0 в математике адаптера
//! (опорные значения и при нулевом времени, пределы uint24, без MAX_FEE, вход без комиссии в бин не больше uint112).
//! Кто вызывает: core/types/pool.rs (PoolState::LbBin), адаптер DEX (pools/load_lb.rs, load_lb20.rs создают,
//! math/lb_swap.rs считает).
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
    /// Решение 445: пул Liquidity Book v2.0 (поля — из feeParameters() и getReservesAndId(); правила математики v2.0).
    /// false — v2.1. Записи без поля (до решения 445) — v2.1.
    #[serde(default)]
    pub v20: bool,
    /// Решение 446 (ревью): слово крючков пула (getLBHooksParameters) на последнем полном чтении — только у вида с
    /// крючками (LB 2.2); None — у вида без крючков. Сверка слота условия пулов адаптера.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks_parameters: Option<[u8; 32]>,
    /// Решение 446 (ревью): обмен через пул исключён адаптером — причина (крючок обмена не допущен или код крючка не
    /// сверен). Состояние пула верное (не «недостоверен»), но котировки и сделки через пул нет. None — обмен разрешён.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_blocked: Option<String>,
}
