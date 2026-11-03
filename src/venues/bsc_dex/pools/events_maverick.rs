//! Файл: venues/bsc_dex/pools/events_maverick.rs
//! Что делает: живое состояние пула Maverick V2 (решение 122 F) по событиям. Любое событие пула, меняющее резервы
//! тиков, активный тик или комиссию (PoolSwap, PoolAddLiquidity, PoolRemoveLiquidity, PoolMigrateBinsUpStack,
//! PoolSetVariableFee), ведёт к Refresh — пул перечитывается целиком (load_maverick.rs: состояние и тики на блоке
//! события). Причина: события несут только суммы обмена/ликвидности, а после обмена пул ещё двигает
//! бины по TWAP (whitepaper V2 draft, §2.5, с. 12-13; moveBins — paraswap-dex-lib maverick-pool-math.ts L229-L298):
//! перенос бинов зависит от TWAP и времени блока — надёжнее перечитать, чем повторять его по событию.
//! Ограничение: flashLoan своего события в интерфейсе не имеет (IMaverickV2Pool.sol L400-L405) — изменение резервов на
//! комиссию займа ловит сверка квотером (решение 73) и перечитывание перед сделкой (решение 83).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: pools/abi_maverick.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

use super::abi_maverick::IMaverickV2Pool as P;
use super::events::Applied;
use super::registry::Entry;

/// topic0 всех событий пула (все ведут к перечитыванию; IMaverickV2Pool.sol L30-L53). Ошибок нет.
pub fn topics() -> Vec<B256> {
    vec![
        P::PoolSwap::SIGNATURE_HASH,
        P::PoolAddLiquidity::SIGNATURE_HASH,
        P::PoolRemoveLiquidity::SIGNATURE_HASH,
        P::PoolMigrateBinsUpStack::SIGNATURE_HASH,
        P::PoolSetVariableFee::SIGNATURE_HASH,
    ]
}

/// topic0 добавления ликвидности — их слушают мелкие пулы (pools/depth.rs). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    vec![P::PoolAddLiquidity::SIGNATURE_HASH]
}

/// Применяет событие: любое известное событие пула → Refresh (см. шапку). Ошибок нет.
pub fn apply(_e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if topics().contains(t) => Applied::Refresh(
            "Maverick V2: событие пула — перечитать состояние, тики и комиссию".into(),
        ),
        _ => Applied::Ignored,
    }
}
