//! Файл: venues/bsc_dex/pools/events_fluid.rs
//! Что делает: события пула Fluid DEX T1 (решение 440) — topic0 для подписки и применение: любое известное событие пула
//! (Swap, внутренний арбитраж LogArbitrage, внесение / снятие / заём / погашение ликвидности, смена комиссии, диапазона,
//! центральной цены, пределов загрузки, пауза) → перечитать пул целиком (load_fluid.rs). Событие Swap несёт только
//! суммы, не резервы; цену пула меняют и операции других пользователей слоя ликвидности (их события — у слоя
//! ликвидности, не у пула), сдвиги по времени и начисление процентов — поэтому вид ещё перечитывается каждым блоком
//! (pools/per_block.rs).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_fluid.rs (topic0 и ссылки на исходник), events.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::abi_fluid;
use super::events::Applied;
use super::registry::Entry;

/// topic0 всех событий пула (abi_fluid.rs; coreModule/events.sol L10-L75, adminModule/events.sol L9-L116). Ошибок нет.
pub fn topics() -> Vec<B256> {
    abi_fluid::topics()
}

/// topic0 событий изменения ликвидности пула. Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    abi_fluid::liquidity_topics()
}

/// Применяет событие: известное событие пула → Refresh, иначе Ignored. Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if topics().contains(t) => Applied::Refresh(format!(
            "{}: событие пула Fluid — перечитать состояние",
            e.family.title
        )),
        _ => Applied::Ignored,
    }
}
