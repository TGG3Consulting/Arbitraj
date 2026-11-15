//! Файл: venues/bsc_dex/pools/events_lb.rs
//! Что делает: события пула Liquidity Book V2.1 (решение 290) — topic0 для подписки и применение. Ревью 290: вместо
//! перечитывания пула целиком (у пары без USDT — все бины, по 2 последовательных вызова на бин) — перечитывание только
//! затронутых бинов (Applied::Bins → listen_log.rs, getBin пачкой — load_lb_bins::many):
//! • Swap — пул выпускает по событию на КАЖДЫЙ пройденный бин (LBPair.sol L499-L531, событие в цикле), id события — бин;
//!   после цикла активный бин = последний пройденный (L550) — активный в состоянии = id события (последнее побеждает);
//! • DepositedToBins / WithdrawnFromBins — ids затронутых бинов (ILBPair.sol L37-L39; LBPair.sol L659, L717);
//!   CompositionFees — бин id (ILBPair.sol L41; при этом mint меняет и параметры комиссии — LBPair.sol L989-L1008);
//! • FlashLoan — комиссия займа ложится в активный бин (LBPair.sol L603-L613, activeId события);
//! • CollectedProtocolFees — забирает накопленную долю протокола, которая в резервах бинов не лежит (swap: бин +
//!   вход − доля протокола, L511-L518; collectProtocolFees бины не трогает, L724-L746) — Ignored;
//! • StaticFeeParametersSet, ForcedDecay — меняют параметры комиссии — перечитать пул целиком (Refresh).
//! Параметры комиссии и активный бин вид перечитывает каждым блоком одной пачкой (per_block_lb.rs): изменчивая часть
//! после обмена (накопитель, опорные значения, время) приходит с ней на блоке события.
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_lb.rs (topic0, поля событий), events.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{B256, U256};

use crate::core::types::PoolState;

use super::abi_lb::{self, ILBPair as P};
use super::events::Applied;
use super::registry::Entry;

/// topic0 всех событий пула (abi_lb.rs; ILBPair.sol L37-L78). Ошибок нет.
pub fn topics() -> Vec<B256> {
    abi_lb::topics()
}

/// topic0 событий изменения ликвидности (DepositedToBins, WithdrawnFromBins). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    abi_lb::liquidity_topics()
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
        Ok(None) if t0 == P::CollectedProtocolFees::SIGNATURE_HASH => Applied::Ignored,
        Ok(None) if topics().contains(&t0) => Applied::Refresh(format!(
            "{}: параметры комиссии пула Liquidity Book изменены — перечитать состояние",
            e.family.title
        )),
        Ok(None) => Applied::Ignored,
        Err(why) => Applied::Refresh(why),
    }
}

/// Затронутые бины события и новый активный бин (только у Swap).
pub type Touched = (Vec<u32>, Option<u32>);

/// Затронутые бины события (без сети; её же зовут тесты на записанных логах). Принимает: topic0, данные лога.
/// Возвращает: (бины, новый активный бин) или None — событие не про бины. Ошибка — данные не по ABI.
pub fn bins_of(
    t0: B256,
    data: &alloy_primitives::LogData,
) -> Result<Option<Touched>, String> {
    let ids = |v: &[U256]| -> Result<Vec<u32>, String> {
        v.iter()
            .map(|x| u32::try_from(*x).map_err(|_| format!("id бина {x} вне uint24")))
            .collect()
    };
    if t0 == P::Swap::SIGNATURE_HASH {
        let s = P::Swap::decode_log_data(data).map_err(|e| format!("Swap не по ABI: {e}"))?;
        let id: u32 = s.id.to();
        return Ok(Some((vec![id], Some(id))));
    }
    if t0 == P::DepositedToBins::SIGNATURE_HASH {
        let m = P::DepositedToBins::decode_log_data(data)
            .map_err(|e| format!("DepositedToBins не по ABI: {e}"))?;
        return Ok(Some((ids(&m.ids)?, None)));
    }
    if t0 == P::WithdrawnFromBins::SIGNATURE_HASH {
        let m = P::WithdrawnFromBins::decode_log_data(data)
            .map_err(|e| format!("WithdrawnFromBins не по ABI: {e}"))?;
        return Ok(Some((ids(&m.ids)?, None)));
    }
    if t0 == P::CompositionFees::SIGNATURE_HASH {
        let c = P::CompositionFees::decode_log_data(data)
            .map_err(|e| format!("CompositionFees не по ABI: {e}"))?;
        return Ok(Some((vec![c.id.to()], None)));
    }
    if t0 == P::FlashLoan::SIGNATURE_HASH {
        let f =
            P::FlashLoan::decode_log_data(data).map_err(|e| format!("FlashLoan не по ABI: {e}"))?;
        return Ok(Some((vec![f.activeId.to()], None)));
    }
    Ok(None)
}
