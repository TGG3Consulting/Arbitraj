//! Файл: core/uncapped_state.rs
//! Что делает: состояние фоновой очереди расчёта «без потолка» (решение 141): по ключу (направление кандидата или id
//! сделки) — когда начинался последний расчёт, идёт ли он и ожидающая работа (повтор сливается: ждёт только последняя);
//! сколько работ ждут (очередь, предел uncapped.queue_max); семафор одновременных расчётов (uncapped.max_concurrent);
//! счётчики для экрана (посчитано, слито, «не посчитано: очередь полна», сбои, время расчёта); открытые эпизоды
//! направлений (uncapped_store.rs). Решение 197: сделки ждут отдельно (deals_waiting, не входят в uncapped.queue_max) и
//! получают место расчёта раньше кандидатов (turn — будит кандидатов, когда ждущих сделок не осталось).
//! Кто вызывает: core/inner.rs (поле Inner::uncapped), uncapped_queue.rs, uncapped_store.rs, uncapped_view.rs.
//! От чего зависит: uncapped_types.rs, frozen.rs (снимок рынка сделки, решение 217), tokio (Semaphore).
//! Настройки: нет (значения читает uncapped_queue.rs).

use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use tokio::sync::Semaphore;

use crate::core::types::Direction;

use super::frozen::Frozen;
use super::uncapped_types::Battle;

/// Ключ работы: кандидат — по направлению (не чаще uncapped.min_interval_ms), сделка — по id (всегда).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    Cand(Direction),
    Deal(String),
}

/// Работа: направление, бой и источник котировок.
#[derive(Debug, Clone)]
pub struct Job {
    pub dir: Direction,
    pub battle: Battle,
    /// Решение 217: None — живые котировки (кандидат); Some(Ok) — снимок рынка момента решения (сделка); Some(Err) —
    /// снимка нет (причина): расчёт по сделке не идёт, живые данные не подставляются.
    pub src: Option<Result<Frozen, String>>,
}

/// Состояние ключа.
#[derive(Debug, Default)]
pub struct Slot {
    pub last: Option<Instant>,
    pub running: bool,
    pub pending: Option<Job>,
}

/// Счётчики для экрана (с запуска процесса).
#[derive(Debug, Default, Clone, Serialize)]
pub struct Stats {
    /// Посчитано расчётов.
    pub done: u64,
    /// Повтор того же ключа заменил ожидающий (слито).
    pub merged: u64,
    /// Не посчитано: очередь полна (uncapped.queue_max; только кандидаты — сделки не выбрасываются, решение 197).
    pub skipped_full: u64,
    /// Решение 197: кандидат посчитан, упущенного нет (упущено 0) — в окно не записан.
    pub no_missed: u64,
    /// Не посчитано: сбой расчёта (последняя причина — last_error).
    pub failed: u64,
    pub last_error: Option<String>,
    /// Время расчёта: сумма и наибольшее, мс.
    pub total_ms: f64,
    pub max_ms: f64,
    /// Сейчас считается и ждёт (очередь).
    pub running: u64,
    pub waiting: u64,
    /// Сделок ждут места расчёта (решение 197).
    pub deals_waiting: u64,
}

/// Открытый эпизод направления: строка в базе, пик упущенного, конец по времени.
#[derive(Debug, Clone)]
pub struct OpenEp {
    pub id: i64,
    pub peak: rust_decimal::Decimal,
    pub last: Instant,
    /// JSON пиковой записи (в ней поле ep обновляется на каждый расчёт эпизода).
    pub body: serde_json::Value,
}

/// Всё состояние очереди.
#[derive(Default)]
pub struct State {
    pub slots: Mutex<HashMap<Key, Slot>>,
    /// Работы кандидатов, которые ещё не начали считаться (ожидающие и ждущие места семафора) — предел uncapped.queue_max.
    pub waiting: AtomicUsize,
    /// Решение 197: сделки, ждущие места расчёта (без предела; пока их больше нуля, место кандидату не отдаётся).
    pub deals_waiting: AtomicUsize,
    /// Будит кандидатов, ждущих своей очереди за сделками.
    pub turn: tokio::sync::Notify,
    /// Семафор одновременных расчётов: (значение uncapped.max_concurrent, семафор).
    pub sem: Mutex<Option<(u64, Arc<Semaphore>)>>,
    pub stats: Mutex<Stats>,
    pub episodes: Mutex<HashMap<Direction, OpenEp>>,
    /// Решение 217: почему по сделке нет записи (сбой расчёта, нет снимка) — id сделки → (когда, причина); читает
    /// сообщение о закрытии (deal_ceiling.rs), старше uncapped.keep_days — убирается (uncapped_store.rs deal_failed).
    pub deal_errors: Mutex<HashMap<String, (chrono::DateTime<chrono::Utc>, String)>>,
}
