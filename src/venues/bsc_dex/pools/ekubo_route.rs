//! Файл: venues/bsc_dex/pools/ekubo_route.rs
//! Что делает: лог ядра Ekubo → пул (решение 447). У ядра PoolId не в topics (события без indexed, обмен — анонимный
//! log0), поэтому общий путь «PoolId = topics[1]» (V4) его не находит: здесь PoolId берётся из данных лога, а у
//! анонимного лога обмена «topic0» — метка events_ekubo::SWAP_MARK. Для логов других видов ответы те же, что раньше
//! (topic0 как есть; ссылки нет).
//! Кто вызывает: pools/session.rs (лог → пул пачки, проверка «событие состояния»), pools/listen_log.rs (лог → запись).
//! От чего зависит: abi_ekubo.rs, events_ekubo.rs.
//! Настройки: нет.
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa: src/Core.sol L840-L847 (log0 обмена: locker 20 байт, затем PoolId),
//! src/interfaces/ICore.sol L111 (PoolInitialized: PoolId — первое слово данных), L119-L126 (PositionUpdated: второе
//! слово), L134 (PositionFeesCollected: второе слово), L141 (FeesAccumulated: первое слово).

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

use crate::core::types::PoolRef;

use super::abi_ekubo::{IEkuboCore as C, decode_swap_log};
use super::events_ekubo::SWAP_MARK;

/// PoolId из лога ядра Ekubo. None — лог не одного из событий пула ядра. Ошибок нет.
pub fn pool_id(log: &Log) -> Option<B256> {
    let data = &log.inner.data.data[..];
    let Some(t0) = log.topic0() else {
        return decode_swap_log(data).map(|(id, _, _)| id);
    };
    let second = [
        C::PositionUpdated::SIGNATURE_HASH,
        C::PositionFeesCollected::SIGNATURE_HASH,
    ];
    let first = [
        C::PoolInitialized::SIGNATURE_HASH,
        C::FeesAccumulated::SIGNATURE_HASH,
    ];
    let at = if second.contains(t0) {
        1
    } else if first.contains(t0) {
        0
    } else {
        return None;
    };
    data.get(at * 32..(at + 1) * 32).map(B256::from_slice)
}

/// Ссылка на пул Ekubo (ядро = адрес лога, PoolId из данных). None — не лог пула ядра. Ошибок нет.
pub fn singleton(log: &Log) -> Option<PoolRef> {
    pool_id(log).map(|id| PoolRef::Singleton {
        manager: log.address(),
        id,
    })
}

/// «topic0» лога: как есть; у анонимного лога обмена ядра (без topics, 116 байт) — SWAP_MARK. Ошибок нет.
pub fn topic(log: &Log) -> Option<B256> {
    match log.topic0() {
        Some(t) => Some(*t),
        None => decode_swap_log(&log.inner.data.data[..]).map(|_| SWAP_MARK),
    }
}
