//! Файл: venues/bsc_dex/pools/events_curve.rs
//! Что делает: живое состояние пула StableSwap (Ellipsis StableSwap; решение 122 F) по событиям. Любое событие,
//! меняющее балансы, A или комиссию (TokenExchange, AddLiquidity, RemoveLiquidity, RemoveLiquidityOne,
//! RemoveLiquidityImbalance, NewFee, RampA, StopRampA), ведёт к Refresh — пул перечитывается целиком (load_curve.rs).
//! Причина: TokenExchange несёт только суммы, а баланс выходной монеты меняется ещё и на долю администратора
//! (`balances[j] = old − dy − dy_admin_fee`, StableSwap.vy L466-L468) — точечно по событию балансы не восстановить.
//! Ограничение: во время плавной смены A (RampA → future_A_time) `A()` меняется с каждым блоком без событий —
//! расхождение ловит сверка с квотером пула (get_dy, решение 73) и перечитывание перед сделкой (решение 83).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: pools/abi_curve.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

use super::abi_curve::{IStableSwapV1, MAX_COINS, topics_for};
use super::events::Applied;
use super::registry::Entry;

/// topic0 всех событий пула (все ведут к перечитыванию); события с массивами — для любого числа монет 2..=MAX_COINS.
/// Ошибок нет.
pub fn topics() -> Vec<B256> {
    let mut out = vec![
        IStableSwapV1::TokenExchange::SIGNATURE_HASH,
        IStableSwapV1::RemoveLiquidityOne::SIGNATURE_HASH,
        IStableSwapV1::NewFee::SIGNATURE_HASH,
        IStableSwapV1::RampA::SIGNATURE_HASH,
        IStableSwapV1::StopRampA::SIGNATURE_HASH,
    ];
    for n in 2..=MAX_COINS {
        let (add, rest) = topics_for(n);
        out.push(add);
        out.extend(rest);
    }
    out
}

/// topic0 добавления ликвидности (AddLiquidity для любого числа монет) — их слушают мелкие пулы (pools/depth.rs).
/// Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    (2..=MAX_COINS).map(|n| topics_for(n).0).collect()
}

/// Применяет событие: любое известное событие пула → Refresh (см. шапку). Ошибок нет.
pub fn apply(_e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if topics().contains(t) => {
            Applied::Refresh("StableSwap: событие пула — перечитать балансы, A и комиссию".into())
        }
        _ => Applied::Ignored,
    }
}
