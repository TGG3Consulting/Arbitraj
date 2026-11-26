//! Файл: venues/bsc_dex/pools/ekubo_route.rs
//! Что делает: лог ядра Ekubo (и расширения TWAMM, решение 454) → пул (решение 447). У ядра PoolId не в topics
//! (события без indexed, обмен — анонимный log0), поэтому общий путь «PoolId = topics[1]» (V4) его не находит: здесь
//! PoolId берётся из данных лога, а у анонимного лога обмена «topic0» — метка events_ekubo::SWAP_MARK, у анонимного
//! VirtualOrdersExecuted TWAMM — events_ekubo::TWAMM_MARK. Для логов других видов ответы те же, что раньше (topic0 как
//! есть; ссылки нет). Разбор по сырым частям лога (адрес, topics, данные) — и для eth_getLogs вне alloy Log
//! (exec/tip_judge.rs, решение 454 — техдолг 447). Лог TWAMM относится к пулу ядра: адрес TWAMM → ядро (bind).
//! Кто вызывает: pools/session.rs (лог → пул пачки, проверка «событие состояния»), pools/listen_log.rs (лог → запись),
//! pools/load_ekubo.rs (bind), exec/tip_judge.rs, check/memory_proof_rules.rs.
//! От чего зависит: abi_ekubo.rs, events_ekubo.rs, gateway/shard_map.rs.
//! Настройки: нет.
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa: src/Core.sol L840-L847 (log0 обмена: locker 20 байт, затем PoolId),
//! src/interfaces/ICore.sol L111 (PoolInitialized: PoolId — первое слово данных), L119-L126 (PositionUpdated: второе
//! слово), L134 (PositionFeesCollected: второе слово), L141 (FeesAccumulated: первое слово); TWAMM —
//! src/extensions/TWAMM.sol L70-L81 (log0 VirtualOrdersExecuted: PoolId — первые 32 байта), ITWAMM.sol L21
//! (OrderUpdated: ключ ордера — слова 3..5; PoolId — abi_ekubo::order_pool_id с адресом лога).

use std::sync::LazyLock;

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256};

use crate::core::types::PoolRef;
use crate::gateway::shard_map::ShardMap;

use super::abi_ekubo::{
    IEkuboCore as C, IEkuboTwamm as T, TWAMM_LOG_LEN, decode_swap_log, order_pool_id,
};
use super::events_ekubo::{SWAP_MARK, TWAMM_MARK};

/// Решение 454: ядро расширения TWAMM (адрес TWAMM → ядро его пулов) — кладёт чтение пула с TWAMM (load_ekubo.rs):
/// лог TWAMM относится к пулу ядра, ссылка пула — (ядро, PoolId).
static TWAMM_CORE: LazyLock<ShardMap<Address, Address>> = LazyLock::new(ShardMap::default);

/// Запомнить ядро расширения TWAMM. Ошибок нет.
pub fn bind(twamm: Address, core: Address) {
    TWAMM_CORE.insert(twamm, core);
}

/// PoolId из сырых частей лога ядра Ekubo или TWAMM. None — лог не одного из событий пула. Ошибок нет.
pub fn pool_id_raw(address: Address, topics: &[B256], data: &[u8]) -> Option<B256> {
    let Some(t0) = topics.first() else {
        if data.len() == TWAMM_LOG_LEN {
            return Some(B256::from_slice(&data[..32]));
        }
        return decode_swap_log(data).map(|(id, _, _)| id);
    };
    if *t0 == T::OrderUpdated::SIGNATURE_HASH {
        let word = |i: usize| data.get(i * 32..(i + 1) * 32).map(B256::from_slice);
        let a = |w: B256| Address::from_slice(&w[12..]);
        return Some(order_pool_id(address, a(word(2)?), a(word(3)?), word(4)?));
    }
    let second = [
        C::PositionUpdated::SIGNATURE_HASH,
        super::ekubo_v2::position_topic(),
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

/// PoolId из лога ядра Ekubo или TWAMM. None — лог не одного из событий пула. Ошибок нет.
pub fn pool_id(log: &Log) -> Option<B256> {
    pool_id_raw(log.address(), log.topics(), &log.inner.data.data[..])
}

/// Ссылка на пул Ekubo (ядро = адрес лога, у лога TWAMM — его ядро из bind; PoolId из данных). None — не лог пула.
/// Ошибок нет.
pub fn singleton(log: &Log) -> Option<PoolRef> {
    let manager = TWAMM_CORE
        .get(&log.address())
        .unwrap_or_else(|| log.address());
    pool_id(log).map(|id| PoolRef::Singleton { manager, id })
}

/// «topic0» лога: как есть; у анонимного лога обмена ядра (116 байт) — SWAP_MARK, у VirtualOrdersExecuted TWAMM
/// (60 байт) — TWAMM_MARK. Ошибок нет.
pub fn topic(log: &Log) -> Option<B256> {
    match log.topic0() {
        Some(t) => Some(*t),
        None if log.inner.data.data.len() == TWAMM_LOG_LEN => Some(TWAMM_MARK),
        None => decode_swap_log(&log.inner.data.data[..]).map(|_| SWAP_MARK),
    }
}
