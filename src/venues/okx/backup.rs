//! Файл: venues/okx/backup.rs
//! Что делает: решение 429 — страховка стакана OKX в памяти: по каждой паре — какой источник сейчас отдан ядру (быстрый
//! локальный стакан канала books или последний снимок канала books5 со второго, отдельного соединения), последний
//! снимок books5 и время последнего отданного ядру стакана по часам биржи (ts). Чистая логика без сети и журнала:
//! решает, отдавать ли кадр ядру, и считает переключения (запись журнала — failover.rs).
//! Кто вызывает: failover.rs (кадры и потери обоих соединений), book.rs (поле Books::backup), book_print.rs (сколько
//! уровней сравнивать), market.rs (новая подписка — clear), тесты.
//! От чего зависит: core/types (OrderBook), book.rs (Alive), gateway/log_clock.rs (часы записи журнала).
//! Настройки: нет (срок свежести снимка передаёт вызывающий — venue.book_stale_ms).
//!
//! Правила (https://www.okx.com/docs-v5/en/, «Order Book Trading → Market Data → WS / Order book channel»): books —
//! «Incremental data will be pushed every 100 ms for the changes in the order book during that period of time»; books5 —
//! «Snapshot data will be pushed every 100 ms when there are changes in the 5 depth levels snapshot»; у обоих ts —
//! «Order book generation time, Unix timestamp format in milliseconds». Связи seqId канала books5 с seqId канала books
//! документация не даёт (prevSeqId «Only applicable to books, books-rpi, books-l2-tbt, books50-l2-tbt»), поэтому
//! «не старее уже отданного» (как решение 109) сравнивается по ts: кадр с ts меньше отданного ядру не отдаётся, равный
//! — отдаётся (то же время построения книги из другого канала). Кадр без ts — сравнить нечем, отдаётся.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::core::types::OrderBook;
use crate::gateway::log_clock::Clock;

use super::book::Alive;

/// Уровней на сторону в снимке books5 — официально «5 depth levels snapshot».
pub const SNAP_LEVELS: usize = 5;

/// Чьё соединение: основное (books или books5 при venue.depth_levels = 5) или страховочное (books5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Main,
    Backup,
}

/// Что сейчас отдано ядру по паре.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Ничего достоверного (старт или потеря без годного снимка).
    #[default]
    None,
    /// Быстрый локальный стакан основного соединения.
    Fast,
    /// Снимок books5 страховочного соединения.
    Snap,
}

/// Последний снимок books5 пары: стакан, живость его соединения, когда пришёл (часы бота, unix мс).
#[derive(Clone)]
pub struct Snap {
    pub book: OrderBook,
    pub alive: Alive,
    pub seen_ms: i64,
}

/// Состояние пары.
#[derive(Default)]
struct Pair {
    mode: Mode,
    /// Быстрый стакан уже был синхронизирован (возврат на него — переключение, а не старт).
    was_fast: bool,
    /// ts последнего отданного ядру стакана (часы биржи, мс).
    served_ms: Option<i64>,
    snap: Option<Snap>,
}

/// Счётчики с прошлой записи журнала (failover.rs обнуляет при записи).
#[derive(Default)]
pub struct Counts {
    /// Быстрый стакан потерял синхронизацию (разрыв prevSeqId, обрыв соединения, отказ подписки, сторож пар).
    pub lost: AtomicU64,
    /// Ядру отдан снимок books5 вместо быстрого стакана.
    pub to_snap: AtomicU64,
    /// Годного снимка не было (нет, старше venue.book_stale_ms или старее отданного) — пара недостоверна.
    pub no_snap: AtomicU64,
    /// Быстрый стакан снова синхронизирован и отдан ядру.
    pub to_fast: AtomicU64,
    /// Страховочное соединение потеряло пару, пока ядро считало по её снимку, — пара недостоверна.
    pub snap_gone: AtomicU64,
}

/// Чем закончилась потеря быстрого стакана.
pub enum Lost {
    /// Отдать ядру этот снимок.
    Serve(Snap),
    /// Ядро уже считает по снимку — ничего не менять.
    Keep,
    /// Годного снимка нет.
    Drop,
}

/// Страховка всех пар площадки. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Backup {
    pairs: Arc<Mutex<HashMap<String, Pair>>>,
    pub counts: Arc<Counts>,
    /// Часы записи журнала (failover.rs).
    pub clock: Arc<Mutex<Clock>>,
}

/// ts стакана, мс.
fn ts(b: &OrderBook) -> Option<i64> {
    b.exchange_time.map(|t| t.timestamp_millis())
}

/// Не старее отданного: (отдано, кадр) — без любого из времён сравнить нечем (true). Ошибок нет.
fn not_older(served: Option<i64>, t: Option<i64>) -> bool {
    match (served, t) {
        (Some(s), Some(t)) => t >= s,
        _ => true,
    }
}

/// Кадр быстрого стакана для состояния пары (см. Backup::fast). Ошибок нет.
fn fast_in(p: &mut Pair, counts: &Counts, t: Option<i64>) -> (bool, bool) {
    let back = p.mode != Mode::Fast && p.was_fast;
    if back {
        inc(&counts.to_fast);
    }
    p.mode = Mode::Fast;
    p.was_fast = true;
    let serve = not_older(p.served_ms, t);
    if serve && t.is_some() {
        p.served_ms = t;
    }
    (serve, back)
}

/// +1 к счётчику.
fn inc(c: &AtomicU64) {
    c.fetch_add(1, Ordering::Relaxed);
}

impl Backup {
    /// Замок карты пар (отравленный — берётся как есть). Ошибок нет.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Pair>> {
        self.pairs.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Новая подписка: состояние пар забыто (счётчики и часы журнала остаются). Ошибок нет.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// Что сейчас отдано ядру по паре. Ошибок нет.
    pub fn mode(&self, ticker: &str) -> Mode {
        self.lock().get(ticker).map_or(Mode::None, |p| p.mode)
    }

    /// Кадр основного соединения изменил стакан пары. Принимает: тикер, ts кадра (мс). Возвращает: (отдать ядру —
    /// не старее отданного; переключение снимки/ничего → быстрый). Ошибок нет.
    pub fn fast(&self, ticker: &str, t: Option<i64>) -> (bool, bool) {
        let mut m = self.lock();
        // Без выделения строки на каждом кадре: вставка — только на первом кадре пары.
        if let Some(p) = m.get_mut(ticker) {
            return fast_in(p, &self.counts, t);
        }
        let p = m.entry(ticker.to_string()).or_default();
        fast_in(p, &self.counts, t)
    }

    /// Быстрый стакан пары потерял синхронизацию. Принимает: тикер, срок свежести снимка (мс; None — не задан: снимок
    /// не отдаётся), сейчас (unix мс). Возвращает: Lost. Ошибок нет.
    pub fn fast_lost(&self, ticker: &str, max_age_ms: Option<i64>, now_ms: i64) -> Lost {
        let mut m = self.lock();
        let p = m.entry(ticker.to_string()).or_default();
        match p.mode {
            Mode::Snap => return Lost::Keep,
            Mode::Fast => inc(&self.counts.lost),
            Mode::None => {}
        }
        let good = p.snap.as_ref().filter(|s| {
            max_age_ms.is_some_and(|a| now_ms - s.seen_ms <= a)
                && not_older(p.served_ms, ts(&s.book))
        });
        match good.cloned() {
            Some(s) => {
                inc(&self.counts.to_snap);
                p.mode = Mode::Snap;
                if let Some(t) = ts(&s.book) {
                    p.served_ms = Some(t);
                }
                Lost::Serve(s)
            }
            None => {
                if p.mode == Mode::Fast {
                    inc(&self.counts.no_snap);
                }
                p.mode = Mode::None;
                Lost::Drop
            }
        }
    }

    /// Кадр страховочного соединения (снимок books5). Принимает: стакан, живость соединения, сейчас (unix мс).
    /// Возвращает: стакан, если его надо отдать ядру (быстрый не синхронизирован и снимок не старее отданного).
    /// Ошибок нет.
    pub fn snap(&self, book: OrderBook, alive: Alive, now_ms: i64) -> Option<OrderBook> {
        let mut m = self.lock();
        let p = m.entry(book.asset.ticker.clone()).or_default();
        let t = ts(&book);
        let serve = p.mode != Mode::Fast && not_older(p.served_ms, t);
        let out = serve.then(|| book.clone());
        if serve {
            if p.mode == Mode::None && p.was_fast {
                inc(&self.counts.to_snap);
            }
            p.mode = Mode::Snap;
            if t.is_some() {
                p.served_ms = t;
            }
        }
        p.snap = Some(Snap {
            book,
            alive,
            seen_ms: now_ms,
        });
        out
    }

    /// Страховочное соединение потеряло пару (обрыв, отказ подписки, неразборчивый кадр). Принимает: тикер.
    /// Возвращает: true — ядро считало по снимку, пару объявить недостоверной. Ошибок нет.
    pub fn snap_lost(&self, ticker: &str) -> bool {
        let mut m = self.lock();
        let Some(p) = m.get_mut(ticker) else {
            return false;
        };
        p.snap = None;
        if p.mode != Mode::Snap {
            return false;
        }
        p.mode = Mode::None;
        inc(&self.counts.snap_gone);
        true
    }
}
