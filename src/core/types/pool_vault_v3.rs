//! Файл: core/types/pool_vault_v3.rs
//! Что делает: решение 452 (T433-F2, доделка Balancer V3 на Arbitrum) — части состояния пула хранилища Balancer V3,
//! которых нет у весового и стейблсвап-пула решения 441: крючок, считающий комиссию обмена (StableSurge, MEV Capture),
//! параметры E-CLP Gyro, состояние LBP (окно продажи, токен проекта, виртуальный баланс резерва). Без названия площадки:
//! только числа, которые передаёт пулу хранилище или отдаёт сам пул. Вынесено из pool_vault.rs (предел размера файла).
//! Решение 474: буфер ERC4626 хранилища (V3Buffer) — сторона пары через буфер: монета пары — базовый токен обёртки,
//! токен пула — обёртка.
//! Кто вызывает: core/types/pool_vault.rs (VaultV3, VaultV3Rule), адаптер DEX (pools/load_balancer_v3*.rs создаёт,
//! math/balancer_v3*.rs, math/gyro_*.rs, math/reclamm.rs считают).
//! От чего зависит: alloy-primitives, serde.
//! Настройки: нет.

use alloy_primitives::{Address, I256, U256};
use serde::{Deserialize, Serialize};

/// Крючок пула, который считает комиссию обмена (Vault.sol 20241204-v3-vault L246-L253: onComputeDynamicSwapFeePercentage
/// вызывается только при флаге пула). Других крючков обмена у принятых пулов нет (beforeSwap / afterSwap — пул не
/// берётся, pools/load_balancer_v3_fixed.rs).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum V3Hook {
    /// Комиссия — статическая комиссия пула.
    #[default]
    None,
    /// StableSurge (balancer-deployments 20250121-v3-stable-surge, 20250403-v3-stable-surge-hook-v2): наибольшая комиссия
    /// и порог дисбаланса пула (getMaxSurgeFeePercentage / getSurgeThresholdPercentage), 18 знаков.
    Surge { max: U256, threshold: U256 },
    /// MEV Capture (20250212-v3-mev-capture-hook): включён ли налог (isMevTaxEnabled), наибольшая комиссия крючка
    /// (getMaxMevSwapFeePercentage), множитель и порог пула (getPoolMevTaxMultiplier / getPoolMevTaxThreshold) и чаевые,
    /// которые крючок увидит у транзакции (tx.gasprice − block.basefee; 0, когда ArbOS не собирает чаевые).
    Mev {
        enabled: bool,
        max: U256,
        multiplier: U256,
        threshold: U256,
        priority: U256,
    },
}

/// Параметры пула Gyro E-CLP (IGyroECLPPool.EclpParams и DerivedEclpParams, getECLPParams): int256, alpha / beta / c / s /
/// lambda — 18 знаков, производные tau, u, v, w, z, dSq — 38 знаков.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EclpParams {
    pub alpha: I256,
    pub beta: I256,
    pub c: I256,
    pub s: I256,
    pub lambda: I256,
    pub tau_alpha_x: I256,
    pub tau_alpha_y: I256,
    pub tau_beta_x: I256,
    pub tau_beta_y: I256,
    pub u: I256,
    pub v: I256,
    pub w: I256,
    pub z: I256,
    pub d_sq: I256,
}

/// Выпуск LBP — у выпусков разный onSwap (balancer-deployments@b55dc2b8 build-info, lbp/LBPool.sol).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LbpVer {
    /// 20250307 и 20250701-v2: весовой расчёт без виртуального баланса (WeightedPool без «+ 1»).
    Plain,
    /// 20251219-v3: виртуальный баланс резерва, проверка «расчёт ≤ реальный резерв» при выходе резерва.
    Virtual,
    /// 20260501-v4: виртуальный баланс резерва и WeightedPool 20260115 («+ 1», нижние балансы); проверка выхода резерва.
    VirtualMin,
}

/// Состояние LBP на блоке чтения: окно продажи (isSwapEnabled — по времени блока), индекс токена проекта, запрет
/// продажи токена проекта в пул, виртуальный баланс резерва (18 знаков; 0 — нет), нижние балансы (только VirtualMin).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LbpState {
    pub ver: LbpVer,
    pub enabled: bool,
    pub project: u8,
    pub block_in: bool,
    pub virt: U256,
    pub min: Vec<U256>,
}

/// Решение 474: буфер ERC4626 хранилища Balancer V3 для токена пула — обёртки (Vault.sol 20241204-v3-vault L1139-L1423).
/// Монета пары — базовый токен обёртки (`underlying`); обмен идёт WRAP → пул → UNWRAP в одном шаге исполнителя. Числа — на
/// блоке чтения, в единицах токенов (сырых).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V3Buffer {
    /// Обёртка ERC4626 — токен пула (ей открывается буфер; адрес шага исполнителя).
    pub wrapped: Address,
    /// Базовый токен буфера (getERC4626BufferAsset хранилища = asset() обёртки).
    pub underlying: Address,
    /// Наименьшая сумма обёртки / развёртки хранилища (getMinimumWrapAmount; вход и выход меньше — откат).
    pub min_wrap: U256,
    /// Опорная сумма превью (настройка dex.bv3_buffer_preview_raw), в единицах того токена, который подаётся в превью.
    pub reference: U256,
    /// previewDeposit(опорная сумма) обёртки: сколько обёртки дают за опорную сумму базового токена.
    pub deposit: U256,
    /// previewRedeem(опорная сумма) обёртки: сколько базового токена дают за опорную сумму обёртки.
    pub redeem: U256,
    /// Базовый токен в буфере (getBufferBalance, первая величина) — предел выхода UNWRAP без внешнего вызова.
    pub underlying_bal: U256,
    /// Обёртка в буфере (getBufferBalance, вторая величина) — предел выхода WRAP без внешнего вызова.
    pub wrapped_bal: U256,
    /// Буферы хранилища на паузе (areBuffersPaused) — обмен через буфер откатится.
    pub paused: bool,
}
