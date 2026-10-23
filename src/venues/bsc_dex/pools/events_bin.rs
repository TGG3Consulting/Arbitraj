//! Файл: venues/bsc_dex/pools/events_bin.rs
//! Что делает: живое состояние пула Infinity Bin: Swap — новый активный бин и перечитывание резервов бинов,
//! через которые прошёл обмен; Mint/Burn/Donate — перечитывание резервов затронутых бинов; изменение комиссий.
//! Резервы перечитываются getBin (факт из блокчейна), а не досчитываются: в Mint суммы включают комиссию
//! композиции, и повторять её у себя — лишний риск расхождения.
//! Кто вызывает: pools/events.rs.
//! От чего зависит: pools/abi_v4.rs, events_cl.rs (комиссии), registry.rs.
//! Настройки: нет.
//!
//! Swap Bin несёт только новый activeId, без резервов — research/flow/bsc_v4.md, раздел 4 («после Swap резервы
//! пройденных бинов надо либо пересчитать локально…, либо перечитать getBin по затронутым id»);
//! Mint/Burn: ids — затронутые бины (IBinPoolManager.sol#L88-L104); Donate(binId) — #L112.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

use crate::core::types::PoolState;

use super::abi_v4::{IInfinityBin, IInfinityFees};
use super::events::Applied;
use super::events_cl::fee_event;
use super::registry::Entry;

/// topic0 событий Bin. Ошибок нет.
pub fn topics() -> Vec<B256> {
    vec![
        IInfinityBin::Swap::SIGNATURE_HASH,
        IInfinityBin::Mint::SIGNATURE_HASH,
        IInfinityBin::Burn::SIGNATURE_HASH,
        IInfinityBin::Donate::SIGNATURE_HASH,
        IInfinityFees::DynamicLPFeeUpdated::SIGNATURE_HASH,
        IInfinityFees::ProtocolFeeUpdated::SIGNATURE_HASH,
    ]
}

/// Применяет событие Bin. Ошибок нет: не разобрано → Refresh.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    if let Some(r) = fee_event(e, log) {
        return r;
    }
    let Some(t0) = log.topic0().copied() else {
        return Applied::Ignored;
    };
    let d = &log.inner.data;
    let ids = |v: &[alloy_primitives::U256]| -> Result<Vec<u32>, String> {
        v.iter()
            .map(|x| u32::try_from(*x).map_err(|_| format!("id бина {x} вне uint24")))
            .collect()
    };
    let res: Result<Vec<u32>, String> = if t0 == IInfinityBin::Swap::SIGNATURE_HASH {
        match IInfinityBin::Swap::decode_log_data(d) {
            Ok(s) => Ok(swap(e, s.activeId.to())),
            Err(err) => Err(format!("Swap не по ABI: {err}")),
        }
    } else if t0 == IInfinityBin::Mint::SIGNATURE_HASH {
        IInfinityBin::Mint::decode_log_data(d)
            .map_err(|err| format!("Mint не по ABI: {err}"))
            .and_then(|m| ids(&m.ids))
    } else if t0 == IInfinityBin::Burn::SIGNATURE_HASH {
        IInfinityBin::Burn::decode_log_data(d)
            .map_err(|err| format!("Burn не по ABI: {err}"))
            .and_then(|b| ids(&b.ids))
    } else if t0 == IInfinityBin::Donate::SIGNATURE_HASH {
        IInfinityBin::Donate::decode_log_data(d)
            .map(|x| vec![x.binId.to()])
            .map_err(|err| format!("Donate не по ABI: {err}"))
    } else {
        return Applied::Ignored;
    };
    match res {
        Ok(v) => Applied::Bins(v),
        Err(why) => Applied::Refresh(why),
    }
}

/// Новый активный бин; возвращает бины для перечитывания: известные непустые между старым и новым и сам новый.
fn swap(e: &mut Entry, new_active: u32) -> Vec<u32> {
    let PoolState::Bin { active_id, .. } = &mut e.pool.state else {
        return vec![new_active];
    };
    let (lo, hi) = if *active_id <= new_active {
        (*active_id, new_active)
    } else {
        (new_active, *active_id)
    };
    *active_id = new_active;
    let mut out: Vec<u32> = e
        .bins
        .as_ref()
        .map(|b| b.reserves.range(lo..=hi).map(|(id, _)| *id).collect())
        .unwrap_or_default();
    if !out.contains(&new_active) {
        out.push(new_active);
    }
    out
}
