//! Файл: venues/bsc_dex/pools/ekubo_keys.rs
//! Что делает: память ключей пулов Ekubo (решение 447): config ключа (расширение, комиссия, вид) по (площадка DEX,
//! PoolId). Ключ неизменяем (PoolId = keccak256 ключа), берётся при проверке пула у PoolKeyIndex (verify_ekubo.rs) и
//! нужен чтению состояния (load_ekubo.rs: вид пула, шаг тика, расширение) до того, как состояние пула прочитано. После
//! перезапуска ключ снова кладёт проверка пула (каждый пул проверяется до подписки). Нет ключа — пул не читается (не
//! гадаем ключ).
//! Кто вызывает: pools/verify_ekubo.rs (remember), pools/load_ekubo.rs (get).
//! От чего зависит: gateway/shard_map.rs.
//! Настройки: нет.

use std::sync::LazyLock;

use alloy_primitives::B256;

use crate::gateway::shard_map::ShardMap;

/// config ключа по (площадка DEX, PoolId): PoolId один и тот же в разных сетях (детерминированное ядро) — ключ с площадкой.
static KEYS: LazyLock<ShardMap<(&'static str, B256), B256>> = LazyLock::new(ShardMap::default);

/// Запомнить config пула (ключ сверен: keccak256 ключа = PoolId). Принимает: площадку, PoolId, config. Ошибок нет.
pub fn remember(venue: &'static str, id: B256, config: B256) {
    KEYS.insert((venue, id), config);
}

/// config пула из памяти. None — пул ещё не проверялся. Ошибок нет.
pub fn get(venue: &'static str, id: B256) -> Option<B256> {
    KEYS.get(&(venue, id))
}
