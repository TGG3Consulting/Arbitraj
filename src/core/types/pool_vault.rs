//! Файл: core/types/pool_vault.rs
//! Что делает: состояние пула «в общем хранилище» (решение 289) — без названия площадки: токены пула лежат в одном
//! контракте-хранилище (Vault), обмен идёт через хранилище по id пула, а формулу считает контракт пула — весовая
//! (произведение балансов в степенях весов) или стейблсвап (инвариант с усилением A). Здесь — балансы всех токенов пула
//! по порядку хранилища, множители точности, комиссия обмена, кривая и индексы пары ребра графа.
//! Вынесено из pool.rs (предел размера файла).
//! Решение 441 (T433-F2): пул хранилища Balancer V3 — то же состояние плюс часть V3 (`VaultPoolData.v3`): живые
//! балансы 18 знаков, курсы токенов, наименьшая сумма обмена хранилища и правило пула (какой фабрикой создан). У пула
//! Balancer V2 часть V3 пуста (None) — его состояние и расчёт прежние.
//! Решение 452: виды Gyro / ReClamm / LBP / QuantAMM и крючки комиссии — новые правила VaultV3Rule, кривая V3 и поля
//! hook / open части V3 (типы — core/types/pool_vault_v3.rs).
//! Решение 473: кривые Balancer V2 других фабрик — VaultCurve::V2 (типы — core/types/pool_vault_v2.rs).
//! Кто вызывает: core/types/pool.rs (PoolState::VaultPool), адаптер DEX (pools/load_balancer.rs создаёт,
//! math/balancer*.rs считает).
//! От чего зависит: alloy-primitives, serde.
//! Настройки: нет.

use alloy_primitives::{B256, U256};
use serde::{Deserialize, Serialize};

pub use super::pool_vault_v2::V2Curve;
pub use super::pool_vault_v3::{EclpParams, LbpState, LbpVer, V3Buffer, V3Hook};

/// Кривая пула хранилища. Математика — в адаптере.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VaultCurve {
    /// Весовой пул: нормированные веса токенов (фикс-точка 18 знаков, сумма = 1e18) по порядку токенов пула.
    Weighted { weights: Vec<U256> },
    /// Стейблсвап: коэффициент усиления A в том виде, в каком его отдаёт пул (A × точность A пула) на блоке чтения.
    Stable { amp: U256 },
    /// Решение 450: весовой пул библиотек v2 (фабрики weighted-pool v2 / v3 / v4: степени 1, 2, 4 — точно, без поправки
    /// на погрешность pow). Поля — как у Weighted.
    WeightedV2 { weights: Vec<U256> },
    /// Решение 450: стейблсвап библиотек v2 (фабрика stable-pool-v2: инвариант по формуле Curve, округление вниз). Поля —
    /// как у Stable.
    StableV2 { amp: U256 },
    /// Решение 452: пул хранилища Balancer V3, чья формула — не весовая и не стейблсвап (Gyro 2-CLP / E-CLP, ReClamm, LBP
    /// с фиксированной ценой): параметры формулы — в правиле части V3 (`VaultV3.rule`). У Balancer V2 не бывает.
    V3,
    /// Решение 473: кривые пулов Balancer V2 других фабрик (composable stable, meta-stable, stable phantom, managed,
    /// linear) — core/types/pool_vault_v2.rs. У Balancer V3 не бывает.
    V2(Box<V2Curve>),
}

/// Состояние пула хранилища на блоке — боксируется в `PoolState::VaultPool`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultPoolData {
    /// Id пула в хранилище (по нему хранилище знает пул; шаг обмена передаёт его хранилищу).
    pub pool_id: B256,
    /// Балансы всех токенов пула в хранилище (по порядку токенов хранилища), в единицах токена.
    pub balances: Vec<U256>,
    /// Множитель точности каждого токена: 10^(18 − decimals) (баланс × множитель — 18 знаков).
    pub scaling: Vec<U256>,
    /// Комиссия обмена — доля над 10^18 (снимается со входа до формулы).
    pub fee: U256,
    /// Кривая пула и её параметры.
    pub curve: VaultCurve,
    /// Обмены пула остановлены (пауза пула): пара пуста для котировки.
    pub paused: bool,
    /// Индекс токена token0 пары среди токенов пула.
    pub i0: u8,
    /// Индекс токена token1 пары среди токенов пула.
    pub i1: u8,
    /// Решение 441: часть хранилища Balancer V3 (None — Balancer V2). У V3 `pool_id` — адрес пула в младших 20 байтах
    /// (id пула V3 — его адрес), `balances` — сырые балансы, `scaling` — множители 10^(18 − decimals) хранилища.
    #[serde(default)]
    pub v3: Option<Box<VaultV3>>,
}

/// Решение 441: часть состояния пула хранилища Balancer V3 на блоке — то, что хранилище передаёт пулу и чем пересчитывает
/// суммы (Vault.sol 20241204-v3-vault L326-L346, L368-L430). Порядок — порядок токенов пула в хранилище.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultV3 {
    /// Живые балансы (getCurrentLiveBalances: сырой баланс × множитель × курс вниз, после сбора доходности), 18 знаков.
    pub live: Vec<U256>,
    /// Курсы токенов (getPoolTokenRates: 1e18 у STANDARD, getRate поставщика у WITH_RATE), 18 знаков.
    pub rates: Vec<U256>,
    /// Наименьшая сумма обмена хранилища в 18 знаках (getMinimumTradeAmount; вход и выход обмена меньше — откат).
    pub min_trade: U256,
    /// Правило пула — по фабрике, создавшей пул (у разных выпусков фабрик onSwap разный).
    pub rule: VaultV3Rule,
    /// Решение 452: крючок, считающий комиссию обмена (StableSurge, MEV Capture); None — статическая комиссия.
    #[serde(default)]
    pub hook: V3Hook,
    /// Решение 452: onSwap пула открыт не только хранилищу (нет onlyVault) — крючок StableSurge зовёт его сам
    /// (StableSurgeHook.sol L136); у пула с onlyVault такой вызов откатывается, обмен невозможен.
    #[serde(default)]
    pub open: bool,
    /// Решение 474: по индексу токена пула — буфер ERC4626, через который идёт эта сторона пары (монета пары — базовый
    /// токен обёртки); пусто или None — сторона пары — сам токен пула.
    #[serde(default)]
    pub via: Vec<Option<V3Buffer>>,
}

/// Решение 441: правило onSwap пула Balancer V3 — по выпуску фабрики (исходники — balancer-deployments@b55dc2b8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VaultV3Rule {
    /// Puzzle Ranges prices a swap on virtual balances and caps output at the spendable real balance.
    PuzzleRanges { virtual_balances: Vec<U256>, enabled: bool },
    /// Весовой пул 20241205-v3-weighted-pool: баланс входа как есть, нижних балансов нет.
    Weighted,
    /// Весовой пул 20260115-v3-weighted-pool-v2: баланс входа + 1 и нижние балансы токенов (getMinTokenBalances, 18
    /// знаков) — баланс входа до и баланс выхода после обмена не ниже своего нижнего.
    WeightedMin { min: Vec<U256> },
    /// Стейблсвап 20241205-v3-stable-pool и 20250324-v3-stable-pool-v2: без проверки дисбаланса.
    Stable,
    /// Стейблсвап 20260116-v3-stable-pool-v3: после обмена наибольший / наименьший баланс < 10 000.
    StableGuard,
    /// Решение 452: Gyro 2-CLP (20250120-v3-gyro-2clp, 20260123-v3-gyro-2clp-v2; кривая V3): √α и √β пула, 18 знаков.
    Gyro2Clp { sqrt_alpha: U256, sqrt_beta: U256 },
    /// Решение 452: Gyro E-CLP (20250124-v3-gyro-eclp, 20260126-v3-gyro-eclp-v2; кривая V3): параметры getECLPParams.
    GyroEclp(Box<EclpParams>),
    /// Решение 452: ReClamm (кривая V3): виртуальные балансы токенов A и B на блоке чтения (18 знаков).
    ReClamm { va: U256, vb: U256 },
    /// Решение 452: LBP (кривая — весовая, веса на блоке чтения): окно продажи, токен проекта, виртуальный резерв.
    Lbp(Box<LbpState>),
    /// Решение 452: LBP с фиксированной ценой (20251205-v3-fixed-price-lbp; кривая V3): окно продажи, индекс токена
    /// проекта, цена токена проекта в резерве (getProjectTokenRate, 18 знаков).
    FixedLbp {
        enabled: bool,
        project: u8,
        rate: U256,
    },
    /// Решение 452: QuantAMM (кривая — весовая, веса на блоке чтения): предел доли сделки maxTradeSizeRatio (18 знаков).
    QuantAmm { max_trade: U256 },
}
