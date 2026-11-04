//! Файл: core/uncapped_state.rs
//! Что делает: состояние фоновой очереди расчёта «без потолка» (решение 141): по ключу (направление кандидата или id
//! сделки) — когда начинался последний расчёт, идёт ли он и ожидающая работа (повтор сливается: ждёт только последняя);
//! сколько работ ждут (очередь, предел uncapped.queue_max); семафор одновременных расчётов (uncapped.max_concurrent);
//! счётчики для экрана (посчитано, слито, «не посчитано: очередь полна», сбои, время расчёта); открытые эпизоды
//! направлений (uncapped_store.rs).
//! Кто вызывает: core/inner.rs (поле Inner::uncapped), uncapped_queue.rs, uncapped_store.rs, uncapped_view.rs.
//! От чего зависит: uncapped_types.rs, tokio (Semaphore).
//! Настройки: нет (значения читает uncapped_queue.rs).

use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use tokio::sync::Semaphore;

use crate::core::types::Direction;

use super::uncapped_types::Battle;

/// Ключ работы: кандидат — по направлению (не чаще uncapped.min_interval_ms), сделка — по id (всегда).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    Cand(Direction),
    Deal(String),
}

/// Работа: направление и бой.
#[derive(Debug, Clone)]
pub struct Job {
    pub dir: Direction,
    pub battle: Battle,
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
    /// Не посчитано: очередь полна (uncapped.queue_max).
    pub skipped_full: u64,
    /// Не посчитано: сбой расчёта (последняя причина — last_error).
    pub failed: u64,
    pub last_error: Option<String>,
    /// Время расчёта: сумма и наибольшее, мс.
    pub total_ms: f64,
    pub max_ms: f64,
    /// Сейчас считается и ждёт (очередь).
    pub running: u64,
    pub waiting: u64,
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
    /// Работы, которые ещё не начали считаться (ожидающие и ждущие места семафора).
    pub waiting: AtomicUsize,
    /// Семафор одновременных расчётов: (значение uncapped.max_concurrent, семафор).
    pub sem: Mutex<Option<(u64, Arc<Semaphore>)>>,
    pub stats: Mutex<Stats>,
    pub episodes: Mutex<HashMap<Direction, OpenEp>>,
}
