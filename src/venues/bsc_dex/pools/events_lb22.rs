//! Файл: venues/bsc_dex/pools/events_lb22.rs
//! Что делает: решение 446 — события пула Liquidity Book V2.2: все события V2.1 (events_lb.rs: Swap, DepositedToBins,
//! WithdrawnFromBins, CompositionFees, CollectedProtocolFees, StaticFeeParametersSet, FlashLoan, ForcedDecay — сигнатуры
//! в V2.2 те же) и HooksParametersSet — смена крючков пула: пул перечитывается целиком (Refresh), а чтение (load.rs →
//! lb_hooks.rs check) снова проверяет крючок обмена по dex.lb_hooks_allowed. Отличие V2.2 у FlashLoan: комиссия займа
//! уходит в долю протокола, а не в активный бин (LBPair.sol L620-L628) — перечитывание активного бина по FlashLoan
//! (events_lb.rs) лишнее, но верное (резерв бина читается с узла как есть).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: events_lb.rs, abi_lb22.rs, events.rs, registry.rs.
//! Настройки: нет.
//!
//! Источник: lfj-gg/joe-v2@v2.2.0 (1297c38) — src/interfaces/ILBPair.sol L38-L78 (события), src/LBPair.sol L846-L861
//! (setHooksParameters: запись _hooksParameters и событие HooksParametersSet — единственное место записи).

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::abi_lb22;
use super::events::Applied;
use super::events_lb;
use super::registry::Entry;

/// topic0 событий пула V2.2: события V2.1 и HooksParametersSet. Ошибок нет.
pub fn topics() -> Vec<B256> {
    let mut t = events_lb::topics();
    t.push(abi_lb22::hooks_set_topic());
    t
}

/// Применяет событие: HooksParametersSet — Refresh (перечитать с проверкой крючков); прочее — как у V2.1. Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    if log.topic0() == Some(&abi_lb22::hooks_set_topic()) {
        return Applied::Refresh(format!(
            "{}: крючки пула Liquidity Book V2.2 изменены — перечитать и проверить крючок обмена",
            e.family.title
        ));
    }
    events_lb::apply(e, log)
}
