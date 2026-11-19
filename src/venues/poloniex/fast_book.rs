//! Файл: venues/poloniex/fast_book.rs
//! Что делает: решение 424 (общая постановка 420–424) — выбор стакана пары для ядра из двух потоков одного соединения
//! (чистая логика, без сети): быстрый — локальный стакан book_lv2 (local_book.rs), страховка — снимки канала book.
//! Пока быстрый синхронизирован — ядру он; разрыв — сразу последний снимок book (если он не старше venue.book_stale_ms
//! и не старше уже отданного), иначе стакан пары недостоверен; после нового снимка book_lv2 — снова быстрый. Пока
//! быстрый не синхронизирован — ядру снимки book, как до решения 424. Счётчики переключений — Stats.
//! Кто вызывает: ws_book.rs, тесты.
//! От чего зависит: local_book.rs, lv2.rs, depth.rs (Depth), core/types.
//! Настройки: нет (venue.book_stale_ms и venue.depth_levels передаёт ws_book.rs).
//!
//! «Не старше уже отданного» (решение 109 по смыслу): у двух каналов свои id, и в документации не сказано, что это одна
//! последовательность, — сравнивать их нельзя; сравнивается createTime (время биржи, есть у обоих каналов): запись с
//! createTime меньше последнего отданного ядру не отдаётся. Нет createTime — отдаётся (как «нет id» у book).
//! Сверка снимка book с быстрым стаканом по версии невозможна (общие id не документированы) — только по содержимому:
//! первые venue.depth_levels уровней совпали — быстрый стакан подтверждён, время кадра пары продлевается (Confirm);
//! не совпали — не разрыв (каналы приходят в разные моменты), ничего.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::core::types::PriceLevel;

use super::depth::Depth;
use super::local_book::{Applied, LocalBook};
use super::lv2::Lv2;

/// Что сделать со стаканом пары.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// Отдать ядру и в память (books.put).
    Publish {
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        time: Option<DateTime<Utc>>,
    },
    /// Быстрый стакан подтверждён снимком book — только в память (продлить время кадра пары), ядру не слать.
    Confirm {
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        time: Option<DateTime<Utc>>,
    },
    /// Стакан пары недостоверен (убрать из памяти, сообщить ядру).
    Invalidate,
    /// Ничего.
    Nothing,
}

/// Счётчики с прошлой записи журнала.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Stats {
    /// Переходов на быстрый стакан (снимок book_lv2 после старта или разрыва).
    pub to_fast: u64,
    /// Переходов с быстрого на снимки book (разрыв при синхронизированном быстром).
    pub to_snap: u64,
    /// Разрывов (lastId, пересечение, неразборчивый кадр).
    pub gaps: u64,
    /// Переподписок book_lv2 ради пересинхронизации.
    pub resyncs: u64,
    /// Причина последнего разрыва.
    pub last_why: String,
}

/// Последний снимок book пары (страховка).
struct Snap {
    bids: Vec<PriceLevel>,
    asks: Vec<PriceLevel>,
    time: Option<DateTime<Utc>>,
    at: Instant,
}

/// Стаканы пар одного соединения.
#[derive(Default)]
pub struct FastBooks {
    local: HashMap<String, LocalBook>,
    snaps: HashMap<String, Snap>,
    /// createTime последнего отданного ядру стакана пары.
    last_out: HashMap<String, DateTime<Utc>>,
    pub stats: Stats,
}

impl FastBooks {
    /// Что делает: запись book_lv2. Принимает: запись, уровней ядру, чтение срока свежести снимка (venue.book_stale_ms;
    /// None — не задан, страховки нет; читается только при разрыве). Возвращает: что сделать и причину разрыва (Some —
    /// нужна переподписка book_lv2). Ошибок нет.
    pub fn on_lv2(
        &mut self,
        d: &Lv2,
        depth: usize,
        stale: impl FnOnce() -> Option<Duration>,
    ) -> (Act, Option<String>) {
        let lb = self.local.entry(d.symbol.clone()).or_default();
        let was = lb.synced();
        match lb.apply(d) {
            Applied::Waiting => (Act::Nothing, None),
            Applied::Gap(why) => self.gap(&d.symbol, was, why, stale),
            Applied::Changed => {
                if !was {
                    self.stats.to_fast += 1;
                }
                let time = lb.time();
                let (bids, asks) = lb.top(depth);
                (self.publish(&d.symbol, bids, asks, time), None)
            }
        }
    }

    /// Что делает: неразборчивый кадр book_lv2 пары — разрыв. Принимает: символ, срок свежести снимка. Возвращает:
    /// как on_lv2. Ошибок нет.
    pub fn on_bad_lv2(
        &mut self,
        symbol: &str,
        why: String,
        stale: impl FnOnce() -> Option<Duration>,
    ) -> (Act, Option<String>) {
        let lb = self.local.entry(symbol.to_string()).or_default();
        let was = lb.synced();
        lb.reset(String::new());
        self.gap(symbol, was, why, stale)
    }

    /// Что делает: новый снимок book (уже прошёл depth::newer). Принимает: снимок (во владение), уровней ядру.
    /// Возвращает: Publish — быстрый не синхронизирован; Confirm — синхронизирован и верх совпал; иначе Nothing.
    /// Ошибок нет.
    pub fn on_book(&mut self, d: Depth, depth: usize) -> Act {
        let snap = Snap {
            bids: d.bids,
            asks: d.asks,
            time: d.exchange_time,
            at: Instant::now(),
        };
        let act = match self.local.get(&d.symbol).filter(|lb| lb.synced()) {
            Some(lb) => {
                let (bids, asks) = lb.top(depth);
                let n = |v: &[PriceLevel]| v.len().min(depth);
                if bids[..] == snap.bids[..n(&snap.bids)] && asks[..] == snap.asks[..n(&snap.asks)]
                {
                    Act::Confirm {
                        bids,
                        asks,
                        time: lb.time(),
                    }
                } else {
                    Act::Nothing
                }
            }
            None => self.publish(&d.symbol, snap.bids.clone(), snap.asks.clone(), snap.time),
        };
        self.snaps.insert(d.symbol, snap);
        act
    }

    /// Что делает: неразборчивый снимок book пары — страховки нет. Принимает: символ. Возвращает: true — быстрый тоже не
    /// синхронизирован (стакан пары недостоверен, как до решения 424). Ошибок нет.
    pub fn on_bad_book(&mut self, symbol: &str) -> bool {
        self.snaps.remove(symbol);
        !self.local.get(symbol).is_some_and(LocalBook::synced)
    }

    /// Что делает: переподписка пары сторожем ядра (оба канала) — оба стакана пары забыты, без счёта разрыва;
    /// createTime последнего отданного помнится (ядру не уйдёт стакан старее). Принимает: символ. Ошибок нет.
    pub fn forget(&mut self, symbol: &str) {
        self.local.remove(symbol);
        self.snaps.remove(symbol);
    }

    /// Разрыв: счёт, страховка или недостоверно. Возвращает: что сделать и причину (переподписка). Ошибок нет.
    fn gap(
        &mut self,
        symbol: &str,
        was: bool,
        why: String,
        stale: impl FnOnce() -> Option<Duration>,
    ) -> (Act, Option<String>) {
        self.stats.gaps += 1;
        if was {
            self.stats.to_snap += 1;
        }
        self.stats.last_why = format!("{symbol}: {why}");
        let stale = stale();
        let fresh = |s: &&Snap| stale.is_some_and(|t| s.at.elapsed() <= t);
        let act = match self.snaps.get(symbol).filter(fresh) {
            Some(s) => {
                let (bids, asks, time) = (s.bids.clone(), s.asks.clone(), s.time);
                match self.publish(symbol, bids, asks, time) {
                    Act::Nothing => Act::Invalidate,
                    a => a,
                }
            }
            None => Act::Invalidate,
        };
        (act, Some(why))
    }

    /// Publish, если createTime не меньше последнего отданного (запоминается); иначе Nothing. Ошибок нет.
    fn publish(
        &mut self,
        symbol: &str,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        time: Option<DateTime<Utc>>,
    ) -> Act {
        if let Some(t) = time {
            if self.last_out.get(symbol).is_some_and(|last| t < *last) {
                return Act::Nothing;
            }
            self.last_out.insert(symbol.to_string(), t);
        }
        Act::Publish { bids, asks, time }
    }
}
