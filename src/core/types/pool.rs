//! Файл: core/types/pool.rs
//! Что делает: типы пулов DEX — вид пула (V2, V3, V4 с концентрированной ликвидностью, V4 Bin, PMM, Solidly,
//! StableSwap, TickBin, PMM от оракула),
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
pub use super::pool_tick_bin::{TickBinData, TickReserves};

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
    /// Solidly (ve(3,3)-форки: Topaz v2, THENA V1; решения 80, 85): пул с резервами и флагом `stable`. Свой контракт
    /// на пул (как V2), но свой Pair-интерфейс (событие Sync с резервами uint256, getAmountOut в самом пуле) — поэтому
    /// не обычный V2. Волатильный пул (stable=false) считается по `x·y=k` (math/v2.rs), стабильный (stable=true) — по
    /// инварианту `x³y+xy³=k` (math/swap_solidly.rs). Какая ветка — по флагу `stable` из состояния.
    Solidly,
    /// StableSwap (инвариант Curve `A·n^n·Σx + D = A·D·n^n + D^(n+1)/(n^n·Πx)`, решение 122 F): N монет в одном
    /// контракте пула, цена — из балансов, коэффициента усиления A и множителей точности монет. Ребро графа — одна
    /// пара монет пула (token0/token1), индексы пары и все балансы — в состоянии.
    StableSwap,
    /// PMM от оракула (решение 122 F, 3): цена — НЕ из резервов пула, а от внешнего оракула (цена, спред, коэффициент
    /// проскальзывания), один общий контракт держит много базовых монет против одной котируемой. Ребро графа — пара
    /// (базовая ↔ котируемая) внутри общего контракта (`PoolRef::Member`). Математика — в адаптере.
    OraclePmm,
    /// Ликвидность по тикам с резервами ОБОИХ токенов в каждом тике (бины разных видов складываются в тик; свой
    /// контракт на пул; решение 122 F, Maverick V2). Комиссия — своя у каждого направления. Своп идёт по тикам от
    /// активного (по возрастанию — вход token0, по убыванию — вход token1). Состояние — TickBinData.
    TickBin,
}

/// Как найти пул в сети.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PoolRef {
    /// V2 и V3: у пула свой адрес контракта.
    Contract(Address),
    /// V4 (CL, Bin): пул живёт в менеджере, ключ — PoolId = keccak256(abi.encode(PoolKey))
    /// (bsc_v4.md, раздел 2 «PoolId»).
    Singleton { manager: Address, id: B256 },
    /// Пара внутри общего контракта, который держит много монет (решение 122 F, 3: PMM от оракула): один адрес —
    /// много рёбер графа, ребро = (контракт, базовая монета). Котируемая монета у контракта одна (token0/token1 пула).
    Member { contract: Address, coin: Address },
}

impl PoolRef {
    /// Адрес контракта, который вызывается для обмена и чтения: у пула со своим контрактом — он, у V4 — менеджер,
    /// у пары общего контракта — общий контракт. Ошибок нет.
    pub fn address(&self) -> Address {
        match self {
            PoolRef::Contract(a) => *a,
            PoolRef::Singleton { manager, .. } => *manager,
            PoolRef::Member { contract, .. } => *contract,
        }
    }
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
    /// Algebra (THENA Fusion и другие форки Algebra V1/V1.9, решения 80, 81): отдельный контракт на пул (как V3),
    /// концентрированная ликвидность с ОДНОЙ ликвидностью (reinvestL у Algebra нет — это Elastic). Комиссия пула
    /// динамическая (плагин волатильности), но внутри одного свопа постоянна — её текущее значение берётся из
    /// `globalState()` (fee, единицы 1e-6). `globalState() → (price, tick, fee, timepointIndex, communityFeeToken0,
    /// communityFeeToken1, unlocked)`, `liquidity() → uint128` — IAlgebraPoolState.sol (@cryptoalgebra/v1.9-core).
    /// Карта тиков читается по СЫРОМУ битмапу `tickTable(int16)` (TickTable индексирует сырой тик, без деления на
    /// tickSpacing) — math/swap_algebra.rs. Математика — math/swap_algebra.rs.
    Algebra {
        sqrt_price_x96: U256,
        tick: i32,
        liquidity: u128,
        /// Текущая комиссия пула в пипсах (единицы 1e-6), из globalState.fee — постоянна на весь своп.
        fee: u32,
    },
    /// PMM DODO (DVM/DPP/DSP, решения 80, 86): состояние кривой с контракта пула (getPMMStateForCall + getUserFeeRate).
    /// Боксируется (`PmmData` много полей U256): без бокса варианта PoolState раздул бы Pool и, через него, горячий
    /// MarketEvent::Pool. Поля — в PmmData. Математика — math/swap_dodo.rs.
    Pmm(Box<PmmData>),
    /// Solidly (Topaz v2, THENA V1; решения 80, 85): резервы и флаг `stable`, плюс масштабы decimals токенов и
    /// комиссия пула — всё, что нужно обеим ветвям математики. `metadata()`/`getReserves()` пула + `getFee(pool, stable)`
    /// фабрики (Pool.sol, PoolFactory.sol, velodrome-finance). `decimals0`/`decimals1` — это 10^decimals токена (как в
    /// контракте: dec0 = 10**decimals0), нужны стабильному инварианту math/swap_solidly.rs для нормализации к 1e18.
    /// `fee` — числитель комиссии над 10000 (как возвращает getFee контракта). stable=false → math/v2.rs,
    /// stable=true → math/swap_solidly.rs.
    Solidly {
        reserve0: U256,
        reserve1: U256,
        stable: bool,
        /// 10^decimals токена0 (контракт: `decimals0 = 10 ** ERC20(token0).decimals()`).
        decimals0: U256,
        /// 10^decimals токена1.
        decimals1: U256,
        /// Комиссия пула — числитель над 10000 (getFee(pool, stable)).
        fee: u32,
    },
    /// StableSwap (решение 122 F): балансы всех монет пула, множители точности, A и комиссия — всё, что нужно
    /// инварианту. Боксируется (много U256), как Pmm. Поля — в StableData.
    StableSwap(Box<StableData>),
    /// Тиковые резервы (решение 122 F, Maverick V2): состояние пула и тики около активного. Боксируется (вектор), как
    /// Pmm. Поля — в TickBinData.
    TickBin(Box<TickBinData>),
    /// PMM от оракула (решение 122 F, 3): состояние оракула и пары общего контракта. Боксируется (много U256), как Pmm.
    /// Поля — в OraclePmmData.
    OraclePmm(Box<OraclePmmData>),
}

/// Состояние пары PMM от оракула (решение 122 F, 3) — боксируется в `PoolState::OraclePmm`. Только то, что читается
/// по официальным интерфейсам: состояние оракула по базовой монете (цена, спред, коэффициент, «цена пригодна»),
/// пауза общего контракта и адрес его реализации (слот прокси EIP-1967). Своей математики обмена нет (развёрнутый код
/// закрыт): выход — по лесенке ответов квотера самого контракта на блоке (точно в точках, между ними — интерполяция).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OraclePmmData {
    /// Цена базовой монеты в котируемой (в десятичных оракула).
    pub price: U256,
    /// Спред, фикс-точка 18 знаков.
    pub spread: U256,
    /// Коэффициент проскальзывания, фикс-точка 18 знаков.
    pub coeff: U256,
    /// Оракул на этом блоке признаёт цену пригодной.
    pub feasible: bool,
    /// Обмены общего контракта приостановлены (paused()).
    pub paused: bool,
    /// Адрес реализации общего контракта за прокси (слот EIP-1967) на этом блоке.
    pub implementation: Address,
    /// Базовая монета пары — token0 пула (иначе token1).
    pub base_is_token0: bool,
    /// Лесенка цен пула (решение 122 F, 3): ответы квотера контракта на блоке — вход котируемой → выход базовой, по
    /// возрастанию входа; пусто — лесенка не задана или квотер не ответил (пара — только сигнал).
    pub ladder_quote_in: Vec<LadderPoint>,
    /// Лесенка: вход базовой → выход котируемой, по возрастанию входа.
    pub ladder_base_in: Vec<LadderPoint>,
}

/// Точка лесенки цен пула: точный вход и ответ квотера контракта (выход) на нём.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LadderPoint {
    pub amount_in: U256,
    pub amount_out: U256,
}

/// Состояние пула StableSwap (решение 122 F) — боксируется в `PoolState::StableSwap`. Математика — в адаптере.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StableData {
    /// Балансы монет пула по порядку монет пула (`balances(k)`), в единицах монеты.
    pub balances: Vec<U256>,
    /// Множители точности монет (`rates[k]`: balance·rate/1e18 — баланс в 18 знаках).
    pub rates: Vec<U256>,
    /// Коэффициент усиления A в том виде, в каком его применяет инвариант пула (`A()` пула).
    pub amp: U256,
    /// Комиссия обмена — числитель над 10^10 (`fee()` пула).
    pub fee: U256,
    /// Точность A (решение 122 F, 3): 1 — A хранится «сырым» (шаблон Curve 2020), 100 — A·A_PRECISION (шаблоны с
    /// A_PRECISION). Формулы D и y делят на неё; при 1 они совпадают с формулами без A_PRECISION.
    pub amp_precision: U256,
    /// Предел раундов итерации Ньютона для D и y (как в контракте пула).
    pub max_rounds: u16,
    /// Контракт откатывает обмен, если итерация не сошлась за max_rounds (иначе — берёт последнее значение).
    pub strict: bool,
    /// Проверка цены пула после обмена против внешнего оракула (решение 122 F, 3): обмен откатывается, если цена
    /// пула после него ушла от цены оракула дальше порога. None — у пула такой проверки нет (или она выключена).
    pub guard: Option<StableGuard>,
    /// Обмены пула остановлены (пауза или kill): пара пуста для котировки.
    pub halted: bool,
    /// Индекс монеты token0 в пуле.
    pub i0: u8,
    /// Индекс монеты token1 в пуле.
    pub i1: u8,
    /// Индекс монеты пула, которая — нативная монета сети (решение «122 X (нативный BNB в стейблсвапах)»): у пула вместо
    /// адреса токена — метка (BNB_ADDRESS 0xEeee… у PancakeSwap Stable / Lista), в паре (token0/token1) — обёртка
    /// (dex.native_wrapped). None — все монеты пула — токены ERC-20.
    #[serde(default)]
    pub native: Option<u8>,
}

/// Проверка цены пула StableSwap после обмена против оракула (решение 122 F, 3) — данные с контракта пула на блоке.
/// Только пул из двух монет. Математика — в адаптере.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StableGuard {
    /// Доля админа в комиссии — числитель над 10^10 (`admin_fee()`): уходит из баланса пула при обмене.
    pub admin_fee: U256,
    /// Цены монет пула от оракула в 18 знаках, по порядку монет пула.
    pub oracle: Vec<U256>,
    /// Пороги отклонения цены монет пула от оракула, фикс-точка 18 знаков, по порядку монет пула.
    pub threshold: Vec<U256>,
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
