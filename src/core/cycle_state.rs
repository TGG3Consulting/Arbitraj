//! Файл: core/cycle_state.rs
//! Что делает: состояние ядра для исполнения циклов DEX ↔ DEX (решение 124): «толчок» поиска (события пулов — tick.rs;
//! толчки, пришедшие во время поиска, сливаются в один повтор, как пересборка справочника) и память последней причины
//! по каждому циклу — чтобы журнал не повторял одно и то же на каждом блоке (запись — только при смене причины;
//! цикл ушёл из сигналов — память о нём стирается; причины на всю площадку — отдельным ключом venue_key, решение 139).
//! Кто вызывает: core/inner.rs (поле cycles), core/tick.rs (kick), core/cycle_watch.rs.
//! От чего зависит: tokio (Notify).
//! Настройки: нет.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use tokio::sync::Notify;

use crate::core::types::DexCycle;

/// Состояние исполнения циклов.
#[derive(Default)]
pub struct CycleState {
    /// Толчок поиска циклов (новые состояния пулов).
    pub kick: Notify,
    /// Ключ цикла → последняя записанная в журнал причина / итог.
    told: Mutex<HashMap<String, String>>,
}

/// Ключ цикла: площадка, базовая монета и пулы по порядку (оба обхода — разные ключи). Ошибок нет.
pub fn key(c: &DexCycle) -> String {
    let pools: Vec<String> = c.hops.iter().map(|h| format!("{:?}", h.pool)).collect();
    format!("{}:{:#x}:{}", c.venue.0, c.base, pools.join(">"))
}

/// Ключ памяти причины на всю площадку, а не на цикл (решение 139: нет кошелька «DEX ↔ DEX», поиск циклов не удался).
/// Не начинается с «площадка:», поэтому retain его не стирает, когда циклы уходят из сигналов. Принимает: площадку,
/// вид причины. Ошибок нет.
pub fn venue_key(v: &crate::core::types::VenueId, what: &str) -> String {
    format!("@{}:{what}", v.0)
}

impl CycleState {
    /// Новый текст по циклу? true — запомнен, писать в журнал; false — тот же, что в прошлый раз. Ошибок нет.
    pub fn fresh(&self, key: &str, text: &str) -> bool {
        let mut m = self.told.lock().unwrap_or_else(|p| p.into_inner());
        if m.get(key).is_some_and(|t| t == text) {
            return false;
        }
        m.insert(key.to_string(), text.to_string());
        true
    }

    /// Забыть циклы, которых больше нет среди кандидатов (площадка — префикс ключа). Ошибок нет.
    pub fn retain(&self, venue: &str, alive: &HashSet<String>) {
        let p = format!("{venue}:");
        self.told
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|k, _| !k.starts_with(&p) || alive.contains(k));
    }
}
