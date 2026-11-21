//! Файл: venues/bsc_dex/pools/events_lb20.rs
//! Что делает: решение 445 — события пула Liquidity Book v2.0 (LFJ V2, Arbitrum): topic0 для подписки и применение.
//! Как у v2.1 (events_lb.rs): затронутые бины перечитываются (Applied::Bins → listen_log.rs → settle_bins.rs: getBin
//! пачкой с активным бином — settle_read.rs), промежуточный активный бин ядру не уходит. События v2.0 — по одному на бин
//! (id — третий индексированный аргумент, ILBPair.sol L99-L140):
//!
//! - Swap — на КАЖДЫЙ пройденный бин с резервом выхода (LBPair.sol L360-L383, событие в цикле); после цикла активный
//!   бин = последний пройденный (L388, L420) — активный в состоянии = id события (последнее побеждает);
//! - DepositedToBin / WithdrawnFromBin — бин id (mint L639, burn L732); CompositionFee — бин id (mint L602, перед
//!   DepositedToBin того же бина);
//! - FlashLoan, FeesCollected, ProtocolFeesCollected бины не меняют: комиссии v2.0 лежат вне резервов бинов
//!   (pairInformation.feesX / feesY, LBPair.sol L470-L480, L756-L833) — не слушаются (abi_lb20.rs topics);
//! - параметры комиссии меняет фабрика без события пары (setFeesParameters, forceDecay — L841-L852): вид перечитывает их
//!   каждым блоком (per_block_lb.rs, lb_head.rs).
//!
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_lb20.rs (topic0, поля событий), events.rs, registry.rs.
//! Настройки: нет.
//!
//! Источник: lfj-gg/joe-v2, тег v2.0.0 (07f10212) — src/interfaces/ILBPair.sol L99-L146, src/LBPair.sol (строки выше).

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{B256, U256};

use crate::core::types::PoolState;

use super::abi_lb20::{self, ILBPairV20 as P};
use super::events::Applied;
use super::events_lb::Touched;
use super::registry::Entry;

/// topic0 слушаемых событий пула (abi_lb20.rs). Ошибок нет.
pub fn topics() -> Vec<B256> {
    abi_lb20::topics()
}

/// topic0 событий изменения ликвидности (DepositedToBin, WithdrawnFromBin). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    abi_lb20::liquidity_topics()
}

/// Применяет событие (см. шапку). Ошибок нет: не разобрано → Refresh.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    let Some(t0) = log.topic0().copied() else {
        return Applied::Ignored;
    };
    match bins_of(t0, &log.inner.data) {
        Ok(Some((ids, active))) => {
            if let (Some(a), PoolState::LbBin(d)) = (active, &mut e.pool.state) {
                d.active_id = a;
            }
            Applied::Bins(ids)
        }
        Ok(None) => Applied::Ignored,
        Err(why) => Applied::Refresh(why),
    }
}

/// Затронутые бины события (без сети; её же зовут тесты на записанных логах). Принимает: topic0, лог. Возвращает:
/// (бины, новый активный бин — только у Swap) или None — событие не про бины. Ошибка — лог не по ABI, id вне uint24.
pub fn bins_of(t0: B256, data: &alloy_primitives::LogData) -> Result<Option<Touched>, String> {
    let id = |x: U256| -> Result<u32, String> {
        u32::try_from(x)
            .ok()
            .filter(|v| *v <= 0xFF_FFFF)
            .ok_or_else(|| format!("id бина {x} вне uint24"))
    };
    if t0 == P::Swap::SIGNATURE_HASH {
        let s = P::Swap::decode_log_data(data).map_err(|e| format!("Swap не по ABI: {e}"))?;
        let i = id(s.id)?;
        return Ok(Some((vec![i], Some(i))));
    }
    if t0 == P::DepositedToBin::SIGNATURE_HASH {
        let m = P::DepositedToBin::decode_log_data(data)
            .map_err(|e| format!("DepositedToBin не по ABI: {e}"))?;
        return Ok(Some((vec![id(m.id)?], None)));
    }
    if t0 == P::WithdrawnFromBin::SIGNATURE_HASH {
        let m = P::WithdrawnFromBin::decode_log_data(data)
            .map_err(|e| format!("WithdrawnFromBin не по ABI: {e}"))?;
        return Ok(Some((vec![id(m.id)?], None)));
    }
    if t0 == P::CompositionFee::SIGNATURE_HASH {
        let c = P::CompositionFee::decode_log_data(data)
            .map_err(|e| format!("CompositionFee не по ABI: {e}"))?;
        return Ok(Some((vec![id(c.id)?], None)));
    }
    Ok(None)
}
