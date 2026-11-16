//! Файл: gateway/core_out_side.rs
//! Что делает: ревью 324 (Б2) — состояние класса общего набора канала ядра (gateway/core_out.rs) ВНЕ замка набора:
//! настройка «откладывать ли» и интервал записи журнала в памяти (gateway/setting_cell.rs — без Store::resolve на кадр),
//! счёт потерь (атомарный — потерянный кадр при незаданных настройках стоит одну атомарную операцию, как прежний
//! dropped += 1) и часы записи журнала. Одно на класс и набор (Feed::side) — задач-подписчиков настроек не больше числа
//! классов, а не по одной на сессию соединения.
//! Кто вызывает: gateway/core_out.rs (хранит), gateway/core_out_book.rs и gateway/book_lost_log.rs (стаканы бирж),
//! venues/bsc_dex/pools/emit_pool.rs (пулы DEX: только «откладывать ли»; счёт потерь пулов — свой, решение 313).
//! От чего зависит: gateway/setting_cell.rs, gateway/log_clock.rs.
//! Настроек нет (имена передаёт вызывающий).

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use super::log_clock::Clock;
use super::setting_cell::Cell;

/// Состояние класса вне замка набора.
#[derive(Debug)]
pub struct Side {
    /// «Откладывать ли» (venue.book_events_keep / dex.pool_events_keep) в памяти.
    pub keep: Arc<Cell>,
    /// Интервал записи журнала, секунды (log.pool_events_lost_interval_s); None — журнал класса пишет не этот модуль.
    pub interval: Option<Arc<Cell>>,
    /// Потеряно с прошлой записи журнала (канал полон, откладывать не велено).
    pub lost: AtomicU64,
    /// Часы записи журнала класса.
    pub clock: Mutex<Clock>,
}

impl Side {
    /// Новое состояние класса. Принимает: ячейку «откладывать ли», ячейку интервала журнала. Ошибок нет.
    pub fn new(keep: Arc<Cell>, interval: Option<Arc<Cell>>) -> Self {
        Self {
            keep,
            interval,
            lost: AtomicU64::new(0),
            clock: Mutex::new(Clock::default()),
        }
    }
}
