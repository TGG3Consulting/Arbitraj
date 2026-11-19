//! Файл: venues/kucoin/fast_state.rs
//! Что делает: решение 423 — состояние быстрого стакана одного соединения KuCoin: локальные стаканы пар, сверки со
//! снимками, какие пары сейчас отдаются ядру из быстрого стакана, снимки REST в пути, паузы после разрыва, последний
//! кадр снимков пары (запас), канал итогов снимков REST.
//! Кто вызывает: ws_conn.rs (создаёт), ws_sess.rs, ws_fast.rs, ws_full.rs.
//! От чего зависит: local_book.rs, fast_check.rs, full_book.rs (Done), core/types.
//! Настройки: пауза — venue.ws_reconnect_pause_ms (ws.rs Cfg.fast).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::core::types::OrderBook;

use super::fast_check::Check;
use super::full_book::Done;
use super::local_book::LocalBook;

/// Решение 423: быстрый стакан соединения.
pub struct Fast {
    pub local: HashMap<String, LocalBook>,
    pub check: HashMap<String, Check>,
    /// Пары, чей стакан сейчас отдаётся ядру из быстрого стакана.
    pub on: HashSet<String>,
    /// Пары, чей снимок REST в очереди или в пути.
    pub pending: HashSet<String>,
    /// После разрыва — новый снимок не раньше (урок 12: без круга пересинхронизаций).
    pub not_before: HashMap<String, Instant>,
    /// Последний кадр снимков пары — запас на случай сбоя быстрого стакана.
    pub snap: HashMap<String, OrderBook>,
    pub tx: mpsc::Sender<Done>,
    /// Пауза повторной пересинхронизации (venue.ws_reconnect_pause_ms).
    pub pause: Duration,
    /// Причина «снимок не просится» уже записана в этой сессии.
    pub warned: bool,
}

impl Fast {
    /// Новый быстрый стакан соединения. Принимает: канал итогов снимков, паузу. Ошибок нет.
    pub fn new(tx: mpsc::Sender<Done>, pause: Duration) -> Self {
        Self {
            local: HashMap::new(),
            check: HashMap::new(),
            on: HashSet::new(),
            pending: HashSet::new(),
            not_before: HashMap::new(),
            snap: HashMap::new(),
            tx,
            pause,
            warned: false,
        }
    }
}
