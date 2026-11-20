//! Файл: venues/bsc_dex/pools/events_shell.rs
//! Что делает: события пула Shell Protocol (Proteus, решение 486) — topic0 для подписки и применение: любое известное
//! событие пула (Swap, Deposit, Withdraw, InitialLiquidity — меняют балансы пула в учёте; ImplementationChanged — другая
//! реализация формулы; PoolFrozen — заморозка) → перечитать пул целиком (load_shell.rs). События несут суммы, а не
//! балансы; комиссия вывода меняется событием учёта (ChangeUnwrapFee) — поэтому вид ещё перечитывается каждым блоком
//! (pools/per_block.rs).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_shell.rs (topic0 и ссылки на исходник), events.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::abi_shell;
use super::events::Applied;
use super::registry::Entry;

/// topic0 всех событий пула (abi_shell.rs; LiquidityPool.sol v3 L67-L70, LiquidityPoolProxy.sol L19-L21). Ошибок нет.
pub fn topics() -> Vec<B256> {
    abi_shell::topics()
}

/// topic0 событий изменения ликвидности пула. Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    abi_shell::liquidity_topics()
}

/// Применяет событие: известное событие пула → Refresh, иначе Ignored. Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if topics().contains(t) => Applied::Refresh(format!(
            "{}: событие пула Shell — перечитать состояние",
            e.family.title
        )),
        _ => Applied::Ignored,
    }
}
