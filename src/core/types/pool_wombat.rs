//! Файл: core/types/pool_wombat.rs
//! Что делает: решение 484 — состояние пула Wombat Exchange (стейблсвап с коэффициентом покрытия) без названия
//! площадки: у каждой монеты пула свой «актив» (Asset) с наличностью cash и обязательствами liability (обе — 18 знаков,
//! uint120), у пула — коэффициент усиления ampFactor (в StableData.amp), комиссия haircutRate и границы комиссии за
//! высокое покрытие startCovRatio / endCovRatio; у динамического пула (DynamicPoolV2) — относительные цены активов
//! getRelativePrice(). Боксируется в `StableData.wombat` (вид ядра — StableSwap); математика — в адаптере
//! (venues/bsc_dex/math/wombat*.rs).
//! Кто вызывает: core/types/pool.rs (StableData), адаптер DEX (pools/load_wombat*.rs создаёт).
//! От чего зависит: alloy-primitives, serde.
//! Настройки: нет.
//!
//! Источник: wombat-exchange/v1-core 2ac067b8 contracts/wombat-core/asset/Asset.sol L33, L36 (cash, liability uint120),
//! pool/PoolV2.sol L48, L51 (ampFactor, haircutRate), pool/HighCovRatioFeePoolV2.sol L16-L17 (startCovRatio,
//! endCovRatio uint128), pool/DynamicPoolV2.sol L27-L34 (_quoteFactor — getRelativePrice активов); развёрнутые
//! реализации Arbitrum — Sourcify (docs/sources/wombat_arbitrum/README.md).

use alloy_primitives::U256;
use serde::{Deserialize, Serialize};

/// Состояние пула Wombat на блоке. Порядок векторов — порядок монет пула getTokens().
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WombatData {
    /// cash() актива каждой монеты, 18 знаков.
    pub cash: Vec<U256>,
    /// liability() актива каждой монеты, 18 знаков.
    pub liability: Vec<U256>,
    /// underlyingTokenDecimals() актива каждой монеты.
    pub decimals: Vec<u8>,
    /// Относительные цены активов getRelativePrice() (18 знаков) — только у динамического пула (DynamicPoolV2);
    /// пусто — пул не динамический (множитель _quoteFactor = 1e18).
    pub price: Vec<U256>,
    /// Актив монеты на паузе (PausableAssets._pausedAssets[монета], слот хранилища пула — Sourcify storageLayout):
    /// обмен ИЗ этой монеты откатится (PoolV2.swap requireAssetNotPaused(fromToken)).
    pub asset_paused: Vec<bool>,
    /// haircutRate пула, 18 знаков (доля выхода).
    pub haircut: U256,
    /// startCovRatio пула, 18 знаков.
    pub start_cov: U256,
    /// endCovRatio пула, 18 знаков.
    pub end_cov: U256,
}
