//! Файл: venues/bsc_dex/pools/events.rs
//! Что делает: общий итог применения события пула и выбор разбора по виду пула; topic0 событий для подписки.
//! Кто вызывает: pools/listen.rs.
//! От чего зависит: events_v2.rs, events_v3.rs, events_cl.rs, events_bin.rs, families.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::families::{Family, Proto};
use super::registry::Entry;
use super::{events_bin, events_cl, events_v2, events_v3};

/// Что произошло с записью после события.
#[derive(Debug, PartialEq, Eq)]
pub enum Applied {
    /// Состояние обновлено — отправить пул ядру.
    Updated,
    /// Событие не меняет нужное нам состояние (например Donate у CL).
    Ignored,
    /// Нужно перечитать резервы этих бинов (Bin) — затем отправить пул ядру.
    Bins(Vec<u32>),
    /// Событие не сходится с картой или не разобрано — пул недостоверен, перечитать целиком.
    Refresh(String),
}

/// topic0 событий, которые слушаются у пулов этого вида. Ошибок нет.
pub fn topics(f: &Family) -> Vec<B256> {
    match f.proto {
        Proto::V2 => events_v2::topics(),
        Proto::PancakeV3 | Proto::UniswapV3 => events_v3::topics(f.proto),
        Proto::InfinityCl | Proto::UniswapV4 => events_cl::topics(f.proto),
        Proto::InfinityBin => events_bin::topics(),
    }
}

/// Применяет событие к записи. Принимает: запись, лог. Возвращает: Applied. Ошибок нет (разбор не удался → Refresh).
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    let r = match e.family.proto {
        Proto::V2 => events_v2::apply(e, log),
        Proto::PancakeV3 | Proto::UniswapV3 => events_v3::apply(e, log),
        Proto::InfinityCl | Proto::UniswapV4 => events_cl::apply(e, log),
        Proto::InfinityBin => events_bin::apply(e, log),
    };
    if let Some(b) = log.block_number
        && matches!(r, Applied::Updated | Applied::Bins(_))
    {
        e.pool.block = b;
        e.synced_block = b;
        e.pool.received_at = chrono::Utc::now();
    }
    r
}
