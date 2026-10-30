//! Файл: venues/bsc_dex/pools/events_elastic.rs
//! Что делает: живое состояние пула KyberSwap Elastic (решение 81) по событиям. Swap несёт цену, baseL и тик после
//! обмена — их и применяем. Mint/Burn/BurnRTokens меняют карту тиков И reinvestL (реинвестирование комиссий), а в
//! самих событиях reinvestL нет — поэтому по ним пул перечитывается целиком (Refresh): reinvestL и карту тиков
//! достоверно даёт только getLiquidityState + читатель тиков (load_elastic.rs).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: pools/abi_elastic.rs, registry.rs.
//! Настройки: нет.
//!
//! События и поля — ks-elastic-sc contracts/interfaces/pool/IPoolEvents.sol#L20-L71 (решение 23):
//! Swap(sender, recipient, deltaQty0, deltaQty1, sqrtP, liquidity=baseL после обмена, currentTick);
//! Mint/Burn(owner, tickLower, tickUpper, qty, qty0, qty1); BurnRTokens(owner, qty, qty0, qty1).
//! reinvestL прирастает комиссией каждого обмена (Pool.sol), но в событие Swap НЕ попадает — между перечитываниями
//! он держится прежним (котировка слегка занижает L = baseL + reinvestL — в безопасную сторону; уточняется в 81c).

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{B256, U256};

use crate::core::types::PoolState;

use super::abi_elastic::IElasticPool;
use super::events::Applied;
use super::registry::Entry;

/// topic0 всех событий пула Elastic. Ошибок нет.
pub fn topics() -> Vec<B256> {
    vec![
        IElasticPool::Swap::SIGNATURE_HASH,
        IElasticPool::Mint::SIGNATURE_HASH,
        IElasticPool::Burn::SIGNATURE_HASH,
        IElasticPool::BurnRTokens::SIGNATURE_HASH,
    ]
}

/// topic0 добавления ликвидности (мелкие пулы слушают только его — pools/depth.rs). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    vec![IElasticPool::Mint::SIGNATURE_HASH]
}

/// Применяет событие Elastic. Ошибок нет: не разобрано — Refresh; событие ликвидности — Refresh (см. шапку).
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    let Some(t0) = log.topic0().copied() else {
        return Applied::Ignored;
    };
    let d = &log.inner.data;
    let bad = |what: &str, err: alloy::sol_types::Error| {
        Applied::Refresh(format!("{what} не по ABI: {err}"))
    };
    if t0 == IElasticPool::Swap::SIGNATURE_HASH {
        return match IElasticPool::Swap::decode_log_data(d) {
            Ok(s) => swap(e, U256::from(s.sqrtP), s.liquidity, s.currentTick.as_i32()),
            Err(err) => bad("Swap", err),
        };
    }
    // Mint / Burn / BurnRTokens меняют карту тиков и/или reinvestL — перечитываем пул целиком (см. шапку).
    if t0 == IElasticPool::Mint::SIGNATURE_HASH {
        return Applied::Refresh("Elastic Mint: перечитать reinvestL и карту тиков".into());
    }
    if t0 == IElasticPool::Burn::SIGNATURE_HASH {
        return Applied::Refresh("Elastic Burn: перечитать reinvestL и карту тиков".into());
    }
    if t0 == IElasticPool::BurnRTokens::SIGNATURE_HASH {
        return Applied::Refresh("Elastic BurnRTokens: перечитать reinvestL".into());
    }
    Applied::Ignored
}

/// Состояние после обмена: цена, baseL и тик (reinvestL событие Swap не несёт — держится прежним, см. шапку).
fn swap(e: &mut Entry, sqrt: U256, base: u128, t: i32) -> Applied {
    match &mut e.pool.state {
        PoolState::Elastic {
            sqrt_price_x96,
            tick,
            base_l,
            ..
        } => {
            *sqrt_price_x96 = sqrt;
            *tick = t;
            *base_l = base;
            Applied::Updated
        }
        _ => Applied::Refresh("состояние пула не Elastic".into()),
    }
}
