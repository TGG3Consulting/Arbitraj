//! Файл: venues/bsc_dex/pools/abi_squad.rs
//! Что делает: решение 281 — ABI событий комиссии SquadSwap V3 и разбор их без сети: события менеджера комиссии
//! SetCustomFee / ToggleCustomFee (поля не индексированы, первое — address pool), UpdatePoolManager фабрики
//! (менеджер индексирован), poolManager() фабрики; фильтры подписки logs и пул из данных события.
//! Кто вызывает: pools/fee_watch.rs; tests/bsc_dex_squadswap_v3_live.rs.
//! От чего зависит: alloy::sol!, families.rs (Family).
//! Настройки: нет (адреса передаёт вызывающий: dex.v3_fee_manager, dex.factory).
//!
//! Источник (verified, Sourcify partial_match, chain 56): SquadV3PoolManager.sol (0x391eaa90f931c6330132efe6c73ebdf77d782ef5,
//! sha256 cbdbe8fd…cdc9) L9, L11, L43-L66; ISquadV3Factory.sol (фабрика 0x10d8612d9d8269e322ab551c18a307cb4d6bc07b)
//! L43, L121; SquadV3Factory.sol (sha256 064bd44a…fc06) L150-L152.

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, LogData};
use serde_json::{Value, json};

use super::families::Family;

alloy::sol! {
    /// SquadV3PoolManager.sol L9, L11 (поля не индексированы).
    interface ISquadV3PoolManager {
        event SetCustomFee(address pool, uint24 fee);
        event ToggleCustomFee(address pool, bool enabled);
    }
    /// ISquadV3Factory.sol L43, L121.
    interface ISquadV3Factory {
        event UpdatePoolManager(address indexed _manager);
        function poolManager() external view returns (address);
    }
}

/// Фильтры: менеджер — SetCustomFee и ToggleCustomFee; фабрика — UpdatePoolManager. Ошибок нет.
pub fn filters(list: &[(&'static Family, Address, Address)]) -> Vec<Value> {
    let mut out = Vec::new();
    for (_, m, fa) in list {
        let t = [
            ISquadV3PoolManager::SetCustomFee::SIGNATURE_HASH,
            ISquadV3PoolManager::ToggleCustomFee::SIGNATURE_HASH,
        ];
        out.push(json!({ "address": m, "topics": [t] }));
        out.push(json!({ "address": fa, "topics": [ISquadV3Factory::UpdatePoolManager::SIGNATURE_HASH] }));
    }
    out
}

/// Событие ли это менеджера комиссии (SetCustomFee / ToggleCustomFee) по topic0. Ошибок нет.
pub fn is_manager_event(topic0: B256) -> bool {
    topic0 == ISquadV3PoolManager::SetCustomFee::SIGNATURE_HASH
        || topic0 == ISquadV3PoolManager::ToggleCustomFee::SIGNATURE_HASH
}

/// Пул из события менеджера (первое поле данных — address pool у обоих событий). Принимает: topic0, данные события.
/// Возвращает: None — не событие менеджера или данные не по ABI. Ошибок нет.
pub fn pool_of(topic0: B256, data: &LogData) -> Option<Address> {
    if topic0 == ISquadV3PoolManager::SetCustomFee::SIGNATURE_HASH {
        return ISquadV3PoolManager::SetCustomFee::decode_log_data(data)
            .ok()
            .map(|e| e.pool);
    }
    if topic0 == ISquadV3PoolManager::ToggleCustomFee::SIGNATURE_HASH {
        return ISquadV3PoolManager::ToggleCustomFee::decode_log_data(data)
            .ok()
            .map(|e| e.pool);
    }
    None
}
