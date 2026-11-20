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
//! src/types/poolState.sol L6-L47 (sqrtRatio 96 бит << 160 | tick 32 бита << 128 | liquidity 128 бит),
//! src/types/poolConfig.sol L32-L134 (config), src/types/tickInfo.sol L4-L32 (liquidityNet << 128 | liquidityDelta),
//! src/math/tickBitmap.sol L14-L29 (слово битмапа), src/types/mevCapturePoolState.sol L4-L25.
//! Решение 454: нативный ETH ключа (token0 = адрес 0 — src/math/constants.sol L26) — поле native; расширение TWAMM —
//! EkuboTwamm (src/types/twammPoolState.sol L14-L44, src/types/timeInfo.sol L5-L37, src/math/timeBitmap.sol L7-L21).

use alloy_primitives::{Address, B256, U256};
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
    /// Решение 454: token0 ключа — нативный ETH (адрес 0); в паре пула и графе вместо него — эта обёртка
    /// (dex.native_wrapped). None — оба токена ключа ERC-20.
    #[serde(default)]
    pub native: Option<Address>,
    /// Решение 454: состояние расширения TWAMM пула (None — у пула нет TWAMM).
    #[serde(default)]
    pub twamm: Option<EkuboTwamm>,
}

/// Состояние TWAMM по пулу (решение 454): TwammPoolState, прочитанные слова битмапа времён и TimeInfo их времён.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EkuboTwamm {
    /// lastVirtualOrderExecutionTime (uint32, время по модулю 2^32).
    pub last: u32,
    /// saleRateToken0 (uint112, 32 дробных бита).
    pub rate0: u128,
    /// saleRateToken1 (uint112).
    pub rate1: u128,
    /// Прочитанные слова битмапа времён (слово = время >> 16), по возрастанию; непрочитанное слово — край расчёта.
    pub words: Vec<u64>,
    /// Ненулевые прочитанные слова: (слово, биты).
    pub bitmap: Vec<(u64, U256)>,
    /// TimeInfo установленных битов прочитанных слов: (время, saleRateDeltaToken0, saleRateDeltaToken1), по времени.
    pub times: Vec<(u64, i128, i128)>,
    /// Время, до которого (включительно) котировка может считать виртуальные ордера: прочитаны все слова допустимых
    /// времён до него (pools/load_ekubo_twamm.rs).
    pub until: u64,
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
