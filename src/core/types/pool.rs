//! Файл: core/types/pool.rs
//! Что делает: типы пулов DEX — вид пула (V2, V3, V4 с концентрированной ликвидностью, V4 Bin, PMM, Solidly,
//! StableSwap, TickBin, PMM от оракула, V2 с усилением — решение 287, пара с фиктивными резервами — решение 288, пул в
//! общем хранилище — решение 289, бины с изменчивой комиссией — решение 290, crypto-пул Curve — решение 442,
//! синглтон-ядро Ekubo — решение 447),
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
pub use super::pool_amp::AmpV2Data;
pub use super::pool_basin::{BasinFunction, BasinWellData};
pub use super::pool_crypto::{CryptoData, CryptoMath, CryptoTweak};
pub use super::pool_ekubo::{EkuboData, EkuboMev, EkuboTick, EkuboTwamm};
pub use super::pool_fictive::FictiveData;
pub use super::pool_lb::LbBinData;
pub use super::pool_proteus::{ProteusData, ProteusUtility};
pub use super::pool_smart::{ColReserves, DebtReserves, SmartColDebtData};
pub use super::pool_solidly::{FeeRule, SolidlyFee, SolidlyGate};
pub use super::pool_tick_bin::{TickBinData, TickReserves};
pub use super::pool_vault::{VaultCurve, VaultPoolData};
pub use super::pool_wombat::WombatData;

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
    /// пара монет пула (token0/token1), индексы пары и все балансы — в состоянии. Решение 295: при dex.multi_pair = да
    /// у многомонетного пула — запись на каждую нужную боту пару (`PoolRef::Multi`), иначе одна пара на пул.
    StableSwap,
    /// PMM от оракула (решение 122 F, 3): цена — НЕ из резервов пула, а от внешнего оракула (цена, спред, коэффициент
    /// проскальзывания), один общий контракт держит много базовых монет против одной котируемой. Ребро графа — пара
    /// (базовая ↔ котируемая) внутри общего контракта (`PoolRef::Member`). Математика — в адаптере.
    OraclePmm,
    /// Ликвидность по тикам с резервами ОБОИХ токенов в каждом тике (бины разных видов складываются в тик; свой
    /// контракт на пул; решение 122 F, Maverick V2). Комиссия — своя у каждого направления. Своп идёт по тикам от
    /// активного (по возрастанию — вход token0, по убыванию — вход token1). Состояние — TickBinData.
    TickBin,
    /// V2 с усилением (решение 287): формула x·y=k по ВИРТУАЛЬНЫМ (усиленным) резервам, выход ограничен реальным
    /// резервом, комиссия динамическая (своя на каждом блоке). Свой контракт на пул. Состояние — AmpV2Data.
    AmpV2,
    /// Пара с фиктивными резервами и средней ценой (решение 288): x·y=k по фиктивным резервам, обмен может делиться на
    /// два шага в зависимости от средней цены, которая пересчитывается к времени блока обмена. Свой контракт на пару.
    /// Состояние — FictiveData.
    Fictive,
    /// Пул в общем хранилище (решение 289): токены всех пулов лежат в одном контракте-хранилище, обмен — через
    /// хранилище по id пула, формула — контракта пула (весовая или стейблсвап). N токенов в пуле; индексы пары — в
    /// состоянии. Без dex.multi_pair — ОДНА пара на пул (ревью 289); с dex.multi_pair = да — запись на каждую нужную
    /// пару (`PoolRef::Multi`, решение 295).
    /// Состояние — VaultPoolData.
    VaultPool,
    /// Бины с изменчивой комиссией (решение 290, Liquidity Book V2.1): свой контракт на пул (не менеджер), токен X/Y — по
    /// порядку создателя пула; комиссия = база + изменчивая часть, которая растёт с каждым пройденным бином и затухает
    /// по времени блока. Резервы бинов — в адаптере (как у Bin). Состояние — LbBinData.
    LbBin,
    /// Пул с центральной ценой и мнимыми резервами на заёмной ликвидности (решение 440): ликвидность — в общем слое
    /// как «смарт-залог» и/или «смарт-долг», x·y=k по мнимым резервам каждой части, обмен делится между частями.
    /// Свой контракт на пул. Состояние — SmartColDebtData (core/types/pool_smart.rs).
    SmartColDebt,
    /// Crypto-пул Curve (решение 442: Twocrypto-NG, Tricrypto-NG): 2–3 монеты разной цены в одном контракте, ликвидность
    /// сосредоточена около внутренней цены price_scale, инвариант с A и gamma, комиссия динамическая (mid_fee → out_fee
    /// по удалённости от равновесия). Свой контракт на пул. Состояние — CryptoData.
    Crypto,
    /// Синглтон-ядро Ekubo (решение 447): все пулы — в одном контракте Core, ключ пула (token0, token1, config) —
    /// PoolId = keccak256 ключа; концентрированная ликвидность по тикам (тик — 1e-6, своя сжатая цена sqrtRatio) или
    /// стейблсвап / полный диапазон; комиссия 0.64 со входа; расширение пула (MEVCapture — доплата за пройденные тики).
    /// Состояние — EkuboData (тики и битмап — в нём).
    Ekubo,
    /// Скважина Basin (решение 485): свой контракт на пул из двух токенов, своей комиссии нет, цену задаёт внешняя
    /// функция скважины от резервов и числа LP-токенов (x·y = s² или стейблсвап). Состояние — BasinWellData
    /// (core/types/pool_basin.rs).
    BasinWell,
    /// Кусочная гипербола за общим учётным контрактом (решение 486, Shell Protocol Proteus): токены пулов — в одном
    /// контракте учёта (ERC-1155), обмен — через него (wrap → computeOutputAmount пула → unwrap), формула — внешняя
    /// реализация пула. Состояние — ProteusData (core/types/pool_proteus.rs).
    Proteus,
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
    /// Пара многомонетного пула (решение 295): один контракт пула держит 3 и более монет (Curve 2020 / Ellipsis,
    /// Curve StableSwap-NG, Balancer V2, Saddle / Nerve, PancakeSwap StableSwap) — ребро графа на каждую нужную боту
    /// пару: ссылка = (контракт, пара token0 < token1 = Pool.token0/token1). Строится только у DEX с dex.multi_pair = да
    /// (без неё у этих видов — `Contract`, одна пара на пул, как до решения). Физический пул — `site()`.
    Multi {
        contract: Address,
        token0: Address,
        token1: Address,
    },
}

impl PoolRef {
    /// Адрес контракта, который вызывается для обмена и чтения: у пула со своим контрактом — он, у V4 — менеджер,
    /// у пары общего контракта — общий контракт. Ошибок нет.
    pub fn address(&self) -> Address {
        match self {
            PoolRef::Contract(a) => *a,
            PoolRef::Singleton { manager, .. } => *manager,
            PoolRef::Member { contract, .. } => *contract,
            PoolRef::Multi { contract, .. } => *contract,
        }
    }

    /// Физический пул (решение 295): у пары многомонетного пула — `Contract(контракт)` (все пары одного пула — одна
    /// площадка), у остальных ссылок — та же ссылка. По нему — запреты «один пул — один шаг маршрута» и «общий пул в
    /// двух долях». Ошибок нет.
    pub fn site(&self) -> PoolRef {
        match self {
            PoolRef::Multi { contract, .. } => PoolRef::Contract(*contract),
            other => other.clone(),
        }
    }

    /// Свой контракт пула (решение 295): `Contract(a)` → a, `Multi` → контракт; у V4 (менеджер) и пары общего
    /// контракта WooFi — None. Ошибок нет.
    pub fn own_contract(&self) -> Option<Address> {
        match self {
            PoolRef::Contract(a) => Some(*a),
            PoolRef::Multi { contract, .. } => Some(*contract),
            PoolRef::Singleton { .. } | PoolRef::Member { .. } => None,
        }
    }
}

/// Две заявки одного владельца Carbon. Поля A и B хранятся в исходном сжатом формате.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarbonOrder {
    pub y: u128,
    pub z: u128,
    pub a: u64,
    pub b: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarbonStrategy {
    pub id: U256,
    pub orders: [CarbonOrder; 2],
}

/// Заявки приведены к порядку tokens. Нативный ETH и WETH имеют разные ID пар.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CarbonState {
    pub controller: Address,
    pub tokens: [Address; 2],
    pub native_side: Option<usize>,
    pub fee_ppm: u32,
    pub strategies: Vec<CarbonStrategy>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AntfarmMarket {
    #[serde(default)]
    pub pool: Address,
    pub tokens: [Address; 2],
    pub atf: Address,
    pub tier: u16,
    pub reserves: [U256; 2],
    /// Actual token0, token1 and ATF balances at the pair.
    pub balances: [U256; 3],
    pub dividends: U256,
    pub lp_supply: U256,
    /// Index of the asset priced in ATF and oracle rate in UQ112x112.
    pub oracle: Option<(usize, U256)>,
    #[serde(default)]
    pub oracle_address: Option<Address>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AntfarmFunds {
    Wallet { atf: U256 },
    Vault { output_balance: U256, fee_per_1e18: U256 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AntfarmLimits {
    pub payer: Address,
    pub module: Address,
    pub stock: Option<AntfarmFunds>,
    pub loans: [Option<AntfarmFunds>;2],
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AntfarmBook {
    pub market: AntfarmMarket,
    /// ATF can be acquired with either token. A linked hourly oracle can reprice the primary fee in the same transaction.
    pub funding: Vec<(AntfarmMarket, Option<U256>)>,
    /// One candidate for one payer at one block. Shared market snapshots never persist a wallet's funding limit.
    #[serde(skip)]
    pub proceeds: Option<(u16, AntfarmFunds)>,
    #[serde(skip)]
    pub limits: Option<AntfarmLimits>,
}

/// DAGrow SSP. Резервы хранятся в 18-разрядных единицах контракта, балансы — в исходных ERC20.
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
pub struct DagrowState {
    pub pool:Address,
    pub stable:Address,
    pub dagr:Address,
    pub payer:Address,
    pub reserves:[U256;2],
    pub balances:[U256;2],
    pub stable_scale:U256,
    /// Bps по результату verifySwap указанного payer отдельно для покупки и продажи.
    pub fees:[Option<u16>;2],
}

/// TempleStableAMMRouter. Порядок резервов — TEMPLE, зарегистрированный стейбл.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TempleState {
    pub pair: Address,
    pub router: Address,
    pub temple: Address,
    pub stable: Address,
    pub defend: Address,
    pub reserves: [U256; 2],
    pub balances: [U256; 2],
    pub ratio: Option<[U256; 2]>,
    pub defense_balance: U256,
    pub amm_authorized: bool,
}

/// Umbrae DAMM, снимок в порядке tokenX/tokenY самой пары.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UmbraeDammState {
    pub tokens: [Address; 2],
    pub reserves: [U256; 2],
    /// Физические остатки до вычета накопленных комиссий.
    pub balances: [U256; 2],
    pub fee_side: usize,
    pub base_fee: u16,
    pub max_fee: u16,
    pub protocol_share: u16,
    pub volatility: u16,
    pub last_price: U256,
    pub protocol_fees: U256,
    pub lp_fees: U256,
    pub lp_supply: U256,
    pub lp_index: U256,
}

/// Один ERC20 Mint Club V2 и его собственное обеспечение в общем контракте Bond.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MintClubState {
    /// Канонически проверенный эмитент выпуска и погашения этого ERC20.
    #[serde(default)]
    pub bond: Address,
    pub token: Address,
    pub reserve: Address,
    pub supply: U256,
    pub steps: Vec<(u128, u128)>,
    pub mint_bps: u16,
    pub burn_bps: u16,
    /// Бухгалтерский остаток этого рынка до выплаты royalty за burn.
    pub bonded: U256,
    /// Фактический баланс резервного ERC20 у Bond, общий для рынков.
    pub available: U256,
}

/// Виртуальные запасы кривой запуска и реальные средства, которыми пара может рассчитаться.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VirtualsState {
    pub agent: Address,
    pub asset: Address,
    pub reserves: [U256; 2],
    pub backing: [U256; 2],
    pub invariant: U256,
    pub graduation: Option<U256>,
    pub buy_tax: u8,
    pub sell_tax: u8,
    pub launch_tax: u8,
    pub external_swap: bool,
}

/// Spot reserves, start-of-observation reference, uncovered borrows and actual ERC20 in each direction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmmalgamState {
    pub reserves: [U256; 2],
    pub reference: [U256; 2],
    pub missing: [U256; 2],
    pub cash: [U256; 2],
}

/// Ethereum MotoSwap FeeRouter charges protocol and creator cuts separately on one side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MotoswapState {
    pub fee_router_frozen: bool,
    pub reserves: [U256; 2],
    pub balances: [U256; 2],
    pub lp_bps: u16,
    pub protocol_bps: u16,
    pub creator_bps: u16,
    pub creators: u8,
    pub fee_on_output: [bool; 2],
}

/// Физические и виртуальные остатки Mooniswap в порядке getTokens. Метка блока общая для состояния и тарифов.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MooniswapState {
    pub physical: [Address;2],
    pub tokens: [Address;2],
    pub balances: [U256;2],
    pub addition: [(U256,u64);2],
    pub removal: [(U256,u64);2],
    pub timestamp:u64,
    pub decay:u64,
    pub fee:U256,
    pub slippage_fee:U256,
    pub successor:bool,
}

/// Оба резерва и обе фиксированные платы EtherVista на одном состоянии сети.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EtherVistaState {
    pub reserve0: U256,
    pub reserve1: U256,
    pub balance0: U256,
    pub balance1: U256,
    pub wrapped: Address,
    pub buy_fee: U256,
    pub sell_fee: U256,
}

/// Состояние пары Memebox. Резервы и доступный остаток исключают начисленные, но ещё не выведенные комиссии.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemeboxState {
    pub reserves: [U256; 2],
    pub available: [U256; 2],
    pub physical: [U256; 2],
    pub fee_bps: u16,
    pub protocol_bps: u16,
}

/// Цена за одну неделимую акцию в атомах расчётного токена на каноническом блоке.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokerbotState {
    pub broker: Address,
    pub cash: Address,
    pub share: Address,
    pub hub: Address,
    pub registered: bool,
    pub buying: bool,
    pub selling: bool,
    pub price: U256,
    pub increment: U256,
    pub cash_balance: U256,
    pub share_balance: U256,
    pub base_price: U256,
    pub supply: U256,
    pub average_first: bool,
    pub license_bps: u16,
    pub drift: bool,
}

/// Логические резервы EmpirePair включают долг одного токена. Балансы ERC20 — физическая выплата.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmpireState {
    pub reserves: [U256; 2],
    pub balances: [U256; 2],
    pub debt: [U256; 2],
    /// Модуль учитывает возмещаемый долг. У стандартного шага V5 и Router02 FOT формула опирается на balanceOf.
    pub debt_aware: bool,
    #[serde(default)]
    pub exact_tokens: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandySwapState {
    pub reserves: [U256; 2],
    pub balances: [U256; 2],
    pub payout_bps: u16,
    pub receiver: Address,
}

/// ToonSwap transfers the floored referral and treasury portions independently before Sync.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToonState {
    pub reserves: [U256; 2],
    pub balances: [U256; 2],
    pub rates: [U256; 2],
    pub recipients: [Address; 2],
    pub pool: Address,
}

/// Состояние DeltaSwapPair для ordinary caller. У gammaPool отдельная ставка.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeltaFee {
    pub router: Address,
    pub generation: u64,
    pub block: u64,
    pub liquidity: U256,
    pub liquidity_block: u32,
    pub traded: U256,
    pub sum: U256,
    pub trade_block: u32,
    pub rate: u8,
    pub threshold: u8,
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
    /// Битмап `tickTable(int16)`: у THENA Fusion (код AlgebraV1, решение 463) СЖАТЫЙ (слово = floor(tick /
    /// tickSpacing) >> 8), у Algebra V1.9 Camelot V3 (решение 461) СЫРОЙ (слово = tick >> 8). У THENA Fusion `fee` —
    /// комиссия свопа СЛЕДУЮЩЕГО блока (по оракулу пула, не globalState.fee). Математика — math/swap_algebra.rs.
    /// Решение 444: Algebra V1.9 с направленными комиссиями (Camelot V3) — globalState несёт две комиссии (feeZto,
    /// feeOtz); `fee` — комиссия обмена token0 → token1 (feeZto), `fee_otz` — token1 → token0 (feeOtz). У видов с одной
    /// комиссией `fee_otz` = None (обе стороны — `fee`): сериализация таких пулов прежняя (поле не пишется).
    Algebra {
        sqrt_price_x96: U256,
        tick: i32,
        liquidity: u128,
        /// Комиссия пула в пипсах (единицы 1e-6), постоянна на весь своп: THENA Fusion — комиссия следующего блока
        /// (решение 463), Algebra Integral — по globalState / fee() плагина, Camelot V3 — globalState.feeZto
        /// (комиссия обмена token0 → token1).
        fee: u32,
        /// Решение 444: комиссия обмена token1 → token0 (feeOtz, пипсы) у Algebra V1.9 с направленными комиссиями;
        /// None — одна комиссия на обе стороны (THENA Fusion, Algebra Integral).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fee_otz: Option<u32>,
    },
    /// PMM DODO (DVM/DPP/DSP, решения 80, 86): состояние кривой с контракта пула (getPMMStateForCall + getUserFeeRate).
    /// Боксируется (`PmmData` много полей U256): без бокса варианта PoolState раздул бы Pool и, через него, горячий
    /// MarketEvent::Pool. Поля — в PmmData. Математика — math/swap_dodo.rs.
    Pmm(Box<PmmData>),
    /// Solidly (Topaz v2, THENA V1; решения 80, 85): резервы и флаг `stable`, плюс масштабы decimals токенов и
    /// комиссия пула — всё, что нужно обеим ветвям математики. `metadata()`/`getReserves()` пула + `getFee(pool, stable)`
    /// фабрики (Pool.sol, PoolFactory.sol, velodrome-finance). `decimals0`/`decimals1` — это 10^decimals токена (как в
    /// контракте: dec0 = 10**decimals0), нужны стабильному инварианту math/swap_solidly.rs для нормализации к 1e18.
    /// `fee` — число комиссии с контракта и правило его применения (решение 273: числитель над делителем — getFee
    /// Velodrome/THENA V1, делитель — usdfi). `gate` — ограничения обмена, проверяемые контрактом пары (пауза фабрики,
    /// предел цены газа; решение 273). stable=false → math/v2.rs, stable=true → math/swap_solidly.rs.
    Solidly {
        reserve0: U256,
        reserve1: U256,
        stable: bool,
        /// 10^decimals токена0 (контракт: `decimals0 = 10 ** ERC20(token0).decimals()`).
        decimals0: U256,
        /// 10^decimals токена1.
        decimals1: U256,
        /// Комиссия пула: значение с контракта и правило (core/types/pool_solidly.rs).
        fee: SolidlyFee,
        /// Ограничения обмена пары (пауза фабрики, предел цены газа); по умолчанию — нет.
        #[serde(default)]
        gate: SolidlyGate,
        /// Решение 443: направленная комиссия (пара Camelot V2 — token0FeePercent / token1FeePercent): Some — комиссия
        /// при входе token1, а `fee` — при входе token0; None — одна комиссия `fee` на оба направления (Solidly). Старые
        /// записи без поля читаются как None; None не пишется (запись Solidly побайтно прежняя).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fee1: Option<SolidlyFee>,
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
    /// V2 с усилением (решение 287): реальные и виртуальные резервы и комиссия блока. Боксируется, как Pmm. Поля — в
    /// AmpV2Data (core/types/pool_amp.rs).
    AmpV2(Box<AmpV2Data>),
    /// Пара с фиктивными резервами (решение 288): резервы, фиктивные резервы, средняя цена, комиссии и время блока.
    /// Боксируется, как Pmm. Поля — в FictiveData (core/types/pool_fictive.rs).
    Fictive(Box<FictiveData>),
    /// Пул в общем хранилище (решение 289): id пула, балансы и множители точности токенов, комиссия, кривая, индексы
    /// пары. Боксируется, как Pmm. Поля — в VaultPoolData (core/types/pool_vault.rs).
    VaultPool(Box<VaultPoolData>),
    /// Бины с изменчивой комиссией (решение 290): активный бин, шаг, порядок X/Y, параметры комиссии и время блока.
    /// Боксируется, как Pmm. Поля — в LbBinData (core/types/pool_lb.rs).
    LbBin(Box<LbBinData>),
    /// Пул со смарт-залогом и смарт-долгом (решение 440): резервы частей, центральная цена, комиссия, пределы снятия и
    /// займа, множители точности, слово цен и время блока. Боксируется, как Pmm. Поля — в SmartColDebtData
    /// (core/types/pool_smart.rs).
    SmartColDebt(Box<SmartColDebtData>),
    /// Crypto-пул Curve (решение 442): балансы, множители точности, price_scale, D, A, gamma, комиссия, индексы пары.
    /// Боксируется, как Pmm. Поля — в CryptoData (core/types/pool_crypto.rs).
    Crypto(Box<CryptoData>),
    /// Синглтон-ядро Ekubo (решение 447): config, цена, тик, ликвидность, битмап и тики, MEVCapture, время блока.
    /// Боксируется, как Pmm. Поля — в EkuboData (core/types/pool_ekubo.rs).
    Ekubo(Box<EkuboData>),
    /// Скважина Basin (решение 485): резервы по порядку токенов скважины, число LP-токенов, функция скважины.
    /// Боксируется, как Pmm. Поля — в BasinWellData (core/types/pool_basin.rs).
    BasinWell(Box<BasinWellData>),
    /// Пул Proteus (решение 486): учётный контракт, реализация, id и балансы сторон в учёте, параметры срезов, BASE_FEE,
    /// формула полезности, делитель комиссии вывода. Боксируется, как Pmm. Поля — в ProteusData
    /// (core/types/pool_proteus.rs).
    Proteus(Box<ProteusData>),
    /// Резервы DeltaSwap и входы формулы комиссии протокола на одном блоке.
    DeltaSwap {
        reserve0: U256,
        reserve1: U256,
        fee: Box<DeltaFee>,
    },
    /// Резервы Oasis исключают кэш протокольных сборов. Доля задана в bps от комиссии.
    Oasis { reserve0:U256,reserve1:U256,share:u32,repeatable:bool },
    /// Резервы пары и плата в ETH, уплачиваемая для каждого отдельного обмена.
    EtherVista(Box<EtherVistaState>),
    Mooniswap(Box<MooniswapState>),
    Motoswap(Box<MotoswapState>),
    Ammalgam(Box<AmmalgamState>),
    Virtuals(Box<VirtualsState>),
    MintClub(Box<MintClubState>),
    UmbraeDamm(Box<UmbraeDammState>),
    Carbon(Box<CarbonState>),
    Rubicon(Box<super::rubicon::Book>),
    YieldBasisLev(Box<super::yield_basis::LevState>),
    YieldBasisPool(Box<super::yield_basis::Preview>),
    Llamma(Box<super::llamma::State>),
    Ambient(Box<super::ambient::State>),
    FluidDexLite(Box<super::fluid_dex_lite::State>),
    BalancerV1(Box<super::balancer_v1::State>),
    BancorLegacy(Box<super::bancor_legacy::BancorLegacyState>),
    BancorV3(Box<super::bancor_v3::BancorState>),
    UniswapV1(Box<super::uniswap_v1::UniswapV1State>),
    Temple(Box<TempleState>),
    DefiPlaza(Box<super::defiplaza::DefiPlazaState>),
    Dagrow(Box<DagrowState>),
    Surge(Box<super::surge::SurgeState>),
    Antfarm(Box<AntfarmBook>),
    Bundles(Box<super::bundles::Book>),
    /// Сумма, списанная с CandySwap, включает независимое удержание из выплаты получателю.
    CandySwap(Box<CandySwapState>),
    ToonSwap(Box<ToonState>),
    Memebox(Box<MemeboxState>),
    Empire(Box<EmpireState>),
    Brokerbot(Box<BrokerbotState>),
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
    /// token0 САМОГО пула, когда порядок токенов пула не по адресу (решение 285: пул Metric OMM — обмену нужен его
    /// zeroForOne). None — у вида порядок не нужен (WooFi). `#[serde(default)]` — старые записи эпизодов.
    #[serde(default)]
    pub pool_token0: Option<Address>,
    /// Счёт LP и кредитные хранилища, деньги которых доступны нескольким разным пулам.
    #[serde(default,skip_serializing_if="Vec::is_empty")]
    pub shared_accounts: Vec<Address>,
}

/// Точка лесенки цен пула: точный вход и ответ квотера контракта (выход) на нём.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LadderPoint {
    pub amount_in: U256,
    pub amount_out: U256,
}

/// Состояние пула StableSwap (решение 122 F) — боксируется в `PoolState::StableSwap`. Математика — в адаптере.
/// Debug — pool_stable_debug.rs (решение 472: поле pair_d выводится только при true — снимки прежних видов прежние).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
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
    /// Шаблон Curve StableSwap-NG (решение «122 (Infinity Stable)»): Some — комиссия динамическая и D считается по
    /// формуле NG (адаптер, math/stableswap_ng.rs); None — шаблоны без NG. `#[serde(default)]` — старые записи эпизодов.
    #[serde(default)]
    pub ng: Option<StableNg>,
    /// Индекс монеты пула, которая — нативная монета сети (решение «122 X (нативный BNB в стейблсвапах)»): у пула вместо
    /// адреса токена — метка (BNB_ADDRESS 0xEeee… у PancakeSwap Stable / Lista), в паре (token0/token1) — обёртка
    /// (dex.native_wrapped). None — все монеты пула — токены ERC-20.
    #[serde(default)]
    pub native: Option<u8>,
    /// Шаблон «комиссия со входа» (решение 284): комиссия `fee` (над 10^10) снимается со ВХОДА до расчёта y, y
    /// округляется вверх, выход = (xp[j] − y) / множитель без вычета 1; две монеты, A·A_PRECISION. false — шаблоны с
    /// комиссией с выхода (Curve 2020, NG). Математика — в адаптере. `#[serde(default)]` — старые записи эпизодов.
    #[serde(default)]
    pub input_fee: bool,
    /// Решение 472: D_P пула из двух монет одной строкой `D·D/x0·D/x1/4` (Curve 2021 — 2pool Arbitrum, Plain2Basic
    /// фабрики; адаптер: math/stableswap_v1.rs Template::pair_d). false — прочие шаблоны. `#[serde(default)]` — старые
    /// записи эпизодов.
    #[serde(default)]
    pub pair_d: bool,
    /// Решение 472 (доделка): после расчёта выхода обмен сохраняет цену пула `save_p` — `pack_prices` откатывает
    /// обмен, если цена `_get_p` после него ≥ 2^128 (реализации фабрики Curve с оракулом цены: Plain2BasicEMA,
    /// Plain2ETHEMA; адаптер: math/stable_ema.rs). false — у пула такой проверки нет. `#[serde(default)]` — старые
    /// записи эпизодов.
    #[serde(default)]
    pub ema: bool,
    /// Решение 484: пул Wombat Exchange (core/types/pool_wombat.rs) — Some: выход считает математика Wombat (адаптер,
    /// math/wombat.rs) по cash / liability активов, а не формулы Curve; balances — cash в единицах монеты, rates —
    /// 10^(36 − decimals) (у динамического пула — ещё × относительная цена / 1e18), amp — ampFactor (18 знаков), fee —
    /// haircutRate / 10^8 (только для показа). None — прочие шаблоны. `#[serde(default)]` — старые записи эпизодов.
    #[serde(default)]
    pub wombat: Option<Box<WombatData>>,
}

/// Данные шаблона StableSwap-NG (решение «122 (Infinity Stable)») — с контракта пула на блоке. Курсы монет с внешним
/// оракулом (вид монеты 1 — rate oracle, 3 — ERC4626) уже входят в `StableData::rates` (`stored_rates()` пула на этом
/// блоке): вид монеты у пула не читается (immutable без геттера), поэтому такие пулы перечитываются каждым блоком.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StableNg {
    /// Множитель комиссии вне привязки — числитель над 10^10 (`offpeg_fee_multiplier()`); ≤ 10^10 — комиссия постоянна.
    pub offpeg_fee_multiplier: U256,
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
