//! Файл: core/types/pool_tick_bin.rs
//! Что делает: состояние пула вида TickBin (решение 122 F; Maverick V2 — вид ядра без названия площадки): резервы
//! тиков около активного, комиссии по направлениям, множители масштаба токенов и доля протокола — для порта расчёта
//! обмена команды Maverick. Вынесено из pool.rs (предел размера файла).
//! Кто вызывает: core/types/pool.rs (PoolState::TickBin), адаптер DEX (pools/load_maverick.rs создаёт,
//! math/maverick_swap.rs считает обмен).
//! От чего зависит: alloy-primitives, serde.
//! Настройки: нет.

use alloy_primitives::U256;
use serde::{Deserialize, Serialize};

/// Резервы одного тика (решение 122 F): `getTick(tick)` пула — в D18-масштабе пула (не в единицах токена).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TickReserves {
    pub tick: i32,
    pub reserve0: u128,
    pub reserve1: u128,
    pub total_supply: u128,
}

/// Состояние пула TickBin (решение 122 F) — боксируется в `PoolState::TickBin`. Всё, что нужно расчёту обмена по
/// тикам (порт команды Maverick — math/maverick_swap.rs, в адаптере). Тики вне [tick_lo, tick_hi] не прочитаны: расчёт
/// за ними частичный; внутри — отсутствующий тик пуст.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TickBinData {
    /// Активный тик (`getState().activeTick`).
    pub active_tick: i32,
    /// Резервы пула в единицах токенов (`getState().reserveA/reserveB`; token0 = tokenA).
    pub reserve0: U256,
    pub reserve1: U256,
    /// Доля протокола в комиссии, тысячные (`getState().protocolFeeRatioD3`).
    pub protocol_fee_d3: u8,
    /// Ширина тика: цена края тика 1.0001^(tick·tick_spacing) (`tickSpacing()`).
    pub tick_spacing: U256,
    /// Комиссия обмена по направлению, D18 (1e18 = 100%): вход token0 / вход token1 (`fee(bool tokenAIn)`).
    pub fee0_in: U256,
    pub fee1_in: U256,
    /// Множители «единицы токена → D18 пула» (`tokenAScale()` / `tokenBScale()`).
    pub scale0: U256,
    pub scale1: U256,
    /// Прочитанный диапазон тиков (включительно): активный ± dex.tick_radius.
    pub tick_lo: i32,
    pub tick_hi: i32,
    /// Непустые тики диапазона (`getTicksAroundActive`), по возрастанию тика.
    pub ticks: Vec<TickReserves>,
}
