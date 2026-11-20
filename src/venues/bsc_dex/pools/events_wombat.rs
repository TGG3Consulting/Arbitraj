//! Файл: venues/bsc_dex/pools/events_wombat.rs
//! Что делает: решение 484 — события пула Wombat Exchange: topic0 для подписки и применение — любое известное событие
//! пула (обмен, ввод / вывод ликвидности, пополнение FillPool, смена ampFactor / haircutRate, добавление / удаление
//! актива, пауза пула и актива) → перечитать пул целиком на блоке события (суммы событий не разбираются). Ещё пул
//! перечитывается каждым блоком (pools/per_block.rs needs): mintFee(token) — внешний вызов без события пула (меняет
//! cash и liability актива), setCovRatioFeeParam — без события, цены активов динамического пула (getRelativePrice) —
//! внешние.
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_wombat.rs, events.rs, registry.rs.
//! Настройки: нет.
//!
//! Сигнатуры — abi_wombat.rs (wombat-exchange/v1-core 2ac067b8 PoolV2.sol L82-L114, PausableAssets.sol L13, L18,
//! HighCovRatioFeePoolV2.sol L29-L34 — setCovRatioFeeParam без события, PoolV2.sol L1026-L1028 — mintFee).

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

use super::abi_wombat::IWombatPool as P;
use super::events::Applied;
use super::registry::Entry;

/// topic0 всех событий пула, меняющих состояние. Ошибок нет.
pub fn topics() -> Vec<B256> {
    vec![
        P::Swap::SIGNATURE_HASH,
        P::Deposit::SIGNATURE_HASH,
        P::Withdraw::SIGNATURE_HASH,
        P::FillPool::SIGNATURE_HASH,
        P::SetAmpFactor::SIGNATURE_HASH,
        P::SetHaircutRate::SIGNATURE_HASH,
        P::AssetAdded::SIGNATURE_HASH,
        P::AssetRemoved::SIGNATURE_HASH,
        P::Paused::SIGNATURE_HASH,
        P::Unpaused::SIGNATURE_HASH,
        P::PausedAsset::SIGNATURE_HASH,
        P::UnpausedAsset::SIGNATURE_HASH,
    ]
}

/// topic0 событий изменения ликвидности (их слушают мелкие пулы, pools/depth.rs): Deposit. Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    vec![P::Deposit::SIGNATURE_HASH]
}

/// Применяет событие: известное событие пула → Refresh. Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if topics().contains(t) => Applied::Refresh(format!(
            "{}: событие пула Wombat — перечитать состояние",
            e.family.title
        )),
        _ => Applied::Ignored,
    }
}
