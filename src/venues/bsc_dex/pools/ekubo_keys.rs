//! Файл: venues/bsc_dex/pools/ekubo_keys.rs
//! Что делает: память ключей пулов Ekubo (решение 447): config ключа (расширение, комиссия, вид) и признак «token0 ключа
//! — нативный ETH» (решение 454) по (площадка DEX, PoolId). Ключ неизменяем (PoolId = keccak256 ключа), берётся при
//! проверке пула у PoolKeyIndex (verify_ekubo.rs) или из события PoolInitialized ядра (history_ekubo.rs, решение 454 —
//! PoolId сверен) и нужен чтению состояния (load_ekubo.rs: вид пула, шаг тика, расширение) до того, как состояние пула
//! прочитано. После перезапуска ключ снова кладёт проверка пула или история (каждый пул проверяется до подписки). Нет
//! ключа — пул не читается (не гадаем ключ).
//! Кто вызывает: pools/verify_ekubo.rs, pools/history_ekubo.rs (remember, get), pools/load_ekubo.rs (get).
//! От чего зависит: gateway/shard_map.rs.
//! Настройки: нет.

use std::sync::LazyLock;

use alloy_primitives::B256;

use crate::gateway::shard_map::ShardMap;

/// Ключ пула без токенов (токены — у записи пула).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    /// config ключа.
    pub config: B256,
    /// token0 ключа — нативный ETH (адрес 0; в паре пула — обёртка dex.native_wrapped).
    pub native: bool,
}

/// Ключ по (площадка DEX, PoolId): PoolId один и тот же в разных сетях (детерминированное ядро) — ключ с площадкой.
static KEYS: LazyLock<ShardMap<(&'static str, B256), Key>> = LazyLock::new(ShardMap::default);

/// Запомнить ключ пула (сверен: keccak256 ключа = PoolId). Принимает: площадку, PoolId, ключ. Ошибок нет.
pub fn remember(venue: &'static str, id: B256, key: Key) {
    KEYS.insert((venue, id), key);
}

/// Ключ пула из памяти. None — пул ещё не проверялся. Ошибок нет.
pub fn get(venue: &'static str, id: B256) -> Option<Key> {
    KEYS.get(&(venue, id))
}
