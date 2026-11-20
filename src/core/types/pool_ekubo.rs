//! Файл: core/types/pool_ekubo.rs
//! Что делает: состояние пула «синглтон-ядро Ekubo» (решение 447) — без названия площадки: config ключа пула (расширение,
//! комиссия, вид: концентрированная ликвидность с шагом тика или стейблсвап / полный диапазон), сжатая цена sqrtRatio,
//! тик, ликвидность, прочитанные слова битмапа тиков и инициализированные тики в них (liquidityNet, liquidityDelta),
//! состояние расширения MEVCapture и время блока чтения. Расчёт обмена — в адаптере (math/ekubo_*). Вынесено из pool.rs
//! (предел размера файла).
//! Кто вызывает: core/types/pool.rs (PoolState::Ekubo), адаптер DEX (pools/load_ekubo.rs создаёт, pools/events_ekubo.rs
//! обновляет, math/ekubo_swap.rs считает).
//! От чего зависит: alloy-primitives, serde.
//! Настройки: нет.
//!
//! Поля — по исходникам EkuboProtocol/evm-contracts@61d09daa (развёрнутое ядро Arbitrum, broadcast/DeployAll.s.sol/42161):
//! src/types/poolState.sol L10-L46 (sqrtRatio 96 бит << 160 | tick 32 бита << 128 | liquidity 128 бит),
//! src/types/poolConfig.sol L46-L123 (config), src/types/tickInfo.sol L12-L35 (liquidityNet << 128 | liquidityDelta),
//! src/math/tickBitmap.sol L174-L193 (слово битмапа), src/types/mevCapturePoolState.sol L8-L26.

use alloy_primitives::{B256, U256};
use serde::{Deserialize, Serialize};

/// Состояние пула Ekubo на блоке — боксируется в `PoolState::Ekubo`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EkuboData {
    /// config ключа пула (bytes32): расширение (старшие 20 байт), комиссия 0.64 (8 байт), вид пула (младшие 4 байта).
    pub config: B256,
    /// sqrtRatio в сжатом виде (uint96 SqrtRatio ядра).
    pub sqrt_ratio: U256,
    /// Текущий тик пула (int32).
    pub tick: i32,
    /// Активная ликвидность (uint128).
    pub liquidity: u128,
    /// Прочитанные слова битмапа тиков (первое, последнее — включительно); у стейблсвап-пула не читаются — (0, 0).
    pub words: (u64, u64),
    /// Ненулевые слова битмапа в прочитанном диапазоне: (номер слова, биты), по возрастанию слова.
    pub bitmap: Vec<(u64, U256)>,
    /// Инициализированные тики прочитанных слов, по возрастанию тика.
    pub ticks: Vec<EkuboTick>,
    /// Состояние расширения MEVCapture пула (None — у пула нет этого расширения).
    pub mev: Option<EkuboMev>,
    /// Время блока чтения, секунды Unix (по нему котировка решает, какой tickLast возьмёт MEVCapture).
    pub block_ts: u64,
}

/// Инициализированный тик пула Ekubo: TickInfo ядра.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EkuboTick {
    pub tick: i32,
    /// liquidityNet (uint128): сколько ликвидности ссылается на тик (≠ 0 — тик инициализирован).
    pub net: u128,
    /// liquidityDelta (int128): изменение активной ликвидности при пересечении тика вверх.
    pub delta: i128,
}

/// Состояние расширения MEVCapture по пулу (MEVCapturePoolState: lastUpdateTime << 224 | tickLast).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EkuboMev {
    /// Время (секунды, uint32) последнего обновления расширением.
    pub last_update_time: u32,
    /// Тик пула на начало этой секунды (int32).
    pub tick_last: i32,
}
