//! Файл: venues/bsc_dex/pools/events_algebra.rs
//! Что делает: живое состояние пула Algebra (THENA Fusion и форки Algebra V1/V1.9, решения 80, 81) по событиям. Swap
//! несёт цену, ликвидность и тик после обмена — их и применяем (решение 83: мгновенное зрение графа). Mint/Burn меняют
//! карту тиков (а событие несёт только изменение в своём диапазоне) — по ним пул перечитывается целиком (Refresh):
//! достоверную карту тиков в окне вокруг цены даёт только перечитывание (load_algebra.rs), как у Elastic.
//! Кто вызывает: pools/events.rs.
//! От чего зависит: pools/abi_algebra.rs, registry.rs.
//! Настройки: нет.
//!
//! События и поля — @cryptoalgebra/v1.9-core IAlgebraPoolEvents.sol (решение 23):
//! Swap(sender, recipient, amount0, amount1, price=sqrtPrice после обмена, liquidity после обмена, tick после обмена);
//! Mint(sender, owner, bottomTick, topTick, liquidityAmount, amount0, amount1);
//! Burn(owner, bottomTick, topTick, liquidityAmount, amount0, amount1). Комиссия пула динамическая: её текущее значение
//! в Swap не приходит — между перечитываниями держится прежним (globalState.fee из load_algebra.rs); для сверки с
//! квотером фаза B делает отдельный вызов (решение 73), точечный пересчёт комиссии по событию — кандидат на 81c.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{B256, U256};

use crate::core::types::PoolState;

use super::abi_algebra::IAlgebraPool;
use super::events::Applied;
use super::registry::Entry;

/// topic0 всех событий пула Algebra. Ошибок нет.
pub fn topics() -> Vec<B256> {
    vec![
        IAlgebraPool::Swap::SIGNATURE_HASH,
        IAlgebraPool::Mint::SIGNATURE_HASH,
        IAlgebraPool::Burn::SIGNATURE_HASH,
    ]
}

/// topic0 добавления ликвидности (мелкие пулы слушают только его — pools/depth.rs). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    vec![IAlgebraPool::Mint::SIGNATURE_HASH]
}

/// Применяет событие Algebra. Ошибок нет: не разобрано — Refresh; событие ликвидности — Refresh (см. шапку).
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    let Some(t0) = log.topic0().copied() else {
        return Applied::Ignored;
    };
    let d = &log.inner.data;
    let bad = |what: &str, err: alloy::sol_types::Error| {
        Applied::Refresh(format!("{what} не по ABI: {err}"))
    };
    if t0 == IAlgebraPool::Swap::SIGNATURE_HASH {
        return match IAlgebraPool::Swap::decode_log_data(d) {
            Ok(s) => swap(e, U256::from(s.price), s.liquidity, s.tick.as_i32()),
            Err(err) => bad("Swap", err),
        };
    }
    // Mint / Burn меняют карту тиков — перечитываем пул целиком (см. шапку).
    if t0 == IAlgebraPool::Mint::SIGNATURE_HASH {
        return Applied::Refresh("Algebra Mint: перечитать ликвидность и карту тиков".into());
    }
    if t0 == IAlgebraPool::Burn::SIGNATURE_HASH {
        return Applied::Refresh("Algebra Burn: перечитать ликвидность и карту тиков".into());
    }
    Applied::Ignored
}

/// Состояние после обмена: цена, ликвидность и тик (комиссию Swap не несёт — держится прежней, см. шапку). Ошибок нет.
fn swap(e: &mut Entry, sqrt: U256, liq: u128, t: i32) -> Applied {
    match &mut e.pool.state {
        PoolState::Algebra {
            sqrt_price_x96,
            tick,
            liquidity,
            ..
        } => {
            *sqrt_price_x96 = sqrt;
            *tick = t;
            *liquidity = liq;
            Applied::Updated
        }
        _ => Applied::Refresh("состояние пула не Algebra".into()),
    }
}
