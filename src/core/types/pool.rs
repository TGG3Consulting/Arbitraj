//! Файл: core/types/pool.rs
//! Что делает: типы пулов DEX — вид пула (V2, V3, V4 с концентрированной ликвидностью, V4 Bin),
//! ссылка на пул, состояние пула на уровне «текущая цена и комиссия», крючок V4 и его группа A/B/C.
//! Кто вызывает: адаптер DEX (создаёт), ядро и интерфейс (показывают, учитывают группу крючка).
//! От чего зависит: core/types/market.rs, alloy-primitives.
//! Настройки: нет.
//!
//! Поля — строго по исследованию research/flow/bsc_dex.md (V2, V3) и bsc_v4.md (Infinity CL, Infinity Bin,
//! Uniswap V4). Ядро котировку не считает (её считает адаптер), поэтому здесь только верхнее состояние пула.
//! Карта тиков (V3/CL) и резервы бинов (Bin) живут внутри адаптера DEX: они нужны только его математике.

use alloy_primitives::{Address, B256, U256};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::market::VenueId;

/// Вид пула — по его математике, без названия конкретной DEX (ядро не знает площадок).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PoolKind {
    /// Классический пул: цена из двух остатков (PancakeSwap V2, Uniswap V2).
    V2,
    /// Концентрированная ликвидность V3: отдельный контракт на пул (PancakeSwap V3, Uniswap V3).
    V3,
    /// Концентрированная ликвидность V4: пул внутри менеджера-синглтона, возможен крючок
    /// (PancakeSwap Infinity CL, Uniswap V4).
    Cl,
    /// Пул из бинов (Liquidity Book), внутри менеджера-синглтона (PancakeSwap Infinity Bin).
    Bin,
    /// PMM (Proactive Market Maker) DODO: цена по кривой PMM из резервов и целей, свой контракт на пул
    /// (DVM/DPP/DSP, решения 80, 86). Отдельная математика (math/swap_dodo.rs) — не тики и не бины.
    Pmm,
}

/// Как найти пул в сети.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PoolRef {
    /// V2 и V3: у пула свой адрес контракта.
    Contract(Address),
    /// V4 (CL, Bin): пул живёт в менеджере, ключ — PoolId = keccak256(abi.encode(PoolKey))
    /// (bsc_v4.md, раздел 2 «PoolId»).
    Singleton { manager: Address, id: B256 },
}

/// Верхнее состояние пула: то, что меняется событиями и нужно, чтобы видеть цену и комиссию.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PoolState {
    /// V2: `getReserves()` / событие `Sync(uint112 reserve0, uint112 reserve1)` (bsc_dex.md, раздел 3).
    V2 { reserve0: U256, reserve1: U256 },
    /// V3: `slot0().sqrtPriceX96`, `slot0().tick`, `liquidity()`; событие `Swap` даёт их после обмена
    /// (bsc_dex.md, раздел 3).
    V3 {
        sqrt_price_x96: U256,
        liquidity: u128,
        tick: i32,
    },
    /// CL (Infinity CL и Uniswap V4): `getSlot0(PoolId) → (sqrtPriceX96, tick, protocolFee, lpFee)`
    /// и `getLiquidity(PoolId)` (bsc_v4.md, раздел 4). Комиссии — uint24 в единицах 1e-6;
    /// protocol_fee упакован: нижние 12 бит — zeroForOne, верхние 12 — oneForZero (bsc_v4.md, раздел 2).
    Cl {
        sqrt_price_x96: U256,
        tick: i32,
        liquidity: u128,
        protocol_fee: u32,
        lp_fee: u32,
    },
    /// Bin (Infinity Bin): `getSlot0(PoolId) → (activeId, protocolFee, lpFee)` (bsc_v4.md, раздел 4).
    /// Цена активного бина = (1 + binStep/10_000)^(activeId − 2^23); binStep — в PoolKey (Pool::bin_step).
    Bin {
        active_id: u32,
        protocol_fee: u32,
        lp_fee: u32,
    },
    /// KyberSwap Elastic (решение 81): отдельный контракт на пул (как V3), но ликвидность обмена
    /// L = base_l + reinvest_l (reinvest_l — ликвидность из реинвестированных комиссий, прирастает каждым обменом).
    /// `getPoolState() → (sqrtP, currentTick, nearestCurrentTick, locked)`, `getLiquidityState() → (baseL, reinvestL,
    /// reinvestLLast)`, `swapFeeUnits()` (единицы 1e-5, FEE_UNITS=100000) — IPoolStorage.sol (ks-elastic-sc).
    /// Событие Swap несёт baseL (не reinvestL): reinvest_l обновляется при перечитывании и событиях ликвидности.
    Elastic {
        sqrt_price_x96: U256,
        tick: i32,
        base_l: u128,
        reinvest_l: u128,
        /// Комиссия пула в feeUnits (FEE_UNITS = 100000 = 100%).
        fee_units: u32,
    },
    /// PMM DODO (DVM/DPP/DSP, решения 80, 86): состояние кривой с контракта пула (getPMMStateForCall + getUserFeeRate).
    /// Боксируется (`PmmData` много полей U256): без бокса варианта PoolState раздул бы Pool и, через него, горячий
    /// MarketEvent::Pool. Поля — в PmmData. Математика — math/swap_dodo.rs.
    Pmm(Box<PmmData>),
}

/// Состояние кривой PMM DODO (DVM/DPP/DSP, решения 80, 86) — боксируется в `PoolState::Pmm`.
/// `getPMMStateForCall()` (i, K, B, Q, B0, Q0, R; adjustedTarget уже применён на контракте) и комиссии
/// `getUserFeeRate(user)` (lpFeeRate, mtFeeRate — фикс-точка 18 знаков). Чистый выход = валовой по кривой минус
/// mulFloor(вых, lp) и mulFloor(вых, mt) — как querySellBase/querySellQuote (DVMTrader.sol). IDODOV2 (DODOEX/contractV2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PmmData {
    /// Референсная цена i (quote за base, 18 знаков).
    pub i: U256,
    /// Наклон кривой K (0..=1e18).
    pub k: U256,
    /// Резерв базовой монеты.
    pub b: U256,
    /// Резерв котируемой монеты.
    pub q: U256,
    /// Цель по базовой (после adjustedTarget контракта).
    pub b0: U256,
    /// Цель по котируемой (после adjustedTarget контракта).
    pub q0: U256,
    /// R-состояние: 0 = ONE, 1 = ABOVE_ONE, 2 = BELOW_ONE (PMMPricing.RState).
    pub r: u8,
    /// Комиссия LP (фикс-точка 18 знаков), `_LP_FEE_RATE_`.
    pub lp_fee_rate: U256,
    /// Комиссия мейнтейнера (фикс-точка 18 знаков), `_MT_FEE_RATE_MODEL_.getFeeRate(user)`.
    pub mt_fee_rate: U256,
    /// Базовая монета пула (`_BASE_TOKEN_`) — это token0 (currency0 < currency1). false — базовая = token1.
    /// PMM различает сторону base/quote, а token0/token1 упорядочены по адресу, поэтому ориентация хранится.
    pub base_is_token0: bool,
}

/// Группа крючка V4 (решение 32).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookGroup {
    /// Не трогает обмен и комиссию: крючка нет или у него выключены beforeSwap, afterSwap,
    /// beforeSwapReturnsDelta, afterSwapReturnsDelta, и комиссия не динамическая.
    A,
    /// Меняет комиссию (динамическая комиссия) или вызывается вокруг обмена без изменения суммы
    /// (beforeSwap/afterSwap без ReturnsDelta).
    B,
    /// Может менять сумму обмена: включён beforeSwapReturnsDelta или afterSwapReturnsDelta.
    C,
}

/// Крючок пула V4 и флаги, влияющие на обмен (bsc_v4.md, разделы 3 и 6).
/// Флаги читает адаптер: у Infinity — биты 0–13 поля `parameters` PoolKey, у Uniswap V4 —
/// младшие 14 бит адреса крючка; раскладка разная, здесь — уже приведённые к одному виду.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookInfo {
    /// Адрес крючка; нулевой адрес — крючка нет.
    pub address: Address,
    /// Группа по флагам (считает адаптер по правилу решения 32).
    pub group: HookGroup,
    /// Комиссия пула динамическая (fee == 0x800000 в PoolKey).
    pub dynamic_fee: bool,
    pub before_swap: bool,
    pub after_swap: bool,
    pub before_swap_returns_delta: bool,
    pub after_swap_returns_delta: bool,
    /// Все 14 бит флагов как у площадки — для показа в интерфейсе.
    pub raw_bits: u16,
}

/// Пул DEX с текущим состоянием.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pool {
    /// Площадка (адаптер DEX).
    pub venue: VenueId,
    /// Вид пула у адаптера, например «pancakeswap_v3» — это id области настроек «DEX» (docs/SETTINGS.md).
    pub family: String,
    pub kind: PoolKind,
    pub reference: PoolRef,
    /// currency0 < currency1 по адресу (bsc_v4.md, «Сортировка валют»; так же в V2/V3).
    pub token0: Address,
    pub token1: Address,
    /// Постоянная комиссия пула, единицы 1e-6: V3 — `fee()`, V4 — `PoolKey.fee`.
    /// V2 — своя комиссия пары (прочитана с пары, если у DEX она своя у каждой пары), иначе None — dex.v2_fee_pct DEX.
    /// None у V4 — динамическая комиссия (lp_fee в состоянии).
    pub fee_pips: Option<u32>,
    /// V3 и CL: tickSpacing; иначе None.
    pub tick_spacing: Option<i32>,
    /// Bin: binStep (1 = 0,01%); иначе None.
    pub bin_step: Option<u32>,
    /// Только V4 (CL, Bin); у V2/V3 крючков нет.
    pub hooks: Option<HookInfo>,
    pub state: PoolState,
    /// Номер блока, в котором получено состояние.
    pub block: u64,
    /// Время получения на нашем сервере.
    pub received_at: DateTime<Utc>,
}
