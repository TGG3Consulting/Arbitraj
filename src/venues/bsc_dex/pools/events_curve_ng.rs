//! Файл: venues/bsc_dex/pools/events_curve_ng.rs
//! Что делает: события пула Curve StableSwap-NG (решение 275) — topic0 для подписки и применение: любое известное
//! событие пула (обмен, ликвидность, A, комиссия) → перечитать пул целиком. Суммы событий не разбираются: перечитывание
//! на блоке события точно. Вид ещё перечитывается каждым блоком (курсы оракулов и rebasing-балансы меняются без
//! событий пула — pools/per_block.rs).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_curve_ng.rs (topic0 и ссылки на официальный исходник), events.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::abi_curve_ng;
use super::events::Applied;
use super::registry::Entry;

/// topic0 всех событий пула. Ошибок нет.
pub fn topics() -> Vec<B256> {
    abi_curve_ng::topics()
}

/// topic0 событий изменения ликвидности (мелкие пулы слушают только их, pools/depth.rs). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    abi_curve_ng::liquidity_topics()
}

/// Применяет событие: известное событие пула → Refresh, иначе Ignored. Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if topics().contains(t) => Applied::Refresh(format!(
            "{}: событие пула Curve NG — перечитать состояние",
            e.family.title
        )),
        _ => Applied::Ignored,
    }
}
