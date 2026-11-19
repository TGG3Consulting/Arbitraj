//! Файл: venues/htx/book_mode.rs
//! Что делает: решение 422 — какой стакан пары HTX отдаётся ядру: быстрый (локальный стакан канала /feed
//! market.$symbol.mbp.$levels, fast_pair.rs), пока он синхронизирован, иначе — снимок канала /ws mbp.refresh
//! (страховка, ws_conn.rs). Последний снимок пары хранится всегда (запасной). Разрыв, конец сессии быстрого канала —
//! сразу запасной снимок (fall_back). Сверка по содержимому: снимок, совпавший с быстрым стаканом в первых
//! min(venue.depth_levels, уровни быстрого канала) уровнях, подтверждает быстрый стакан (время кадра пары); быстрый
//! стакан, который дольше venue.book_stale_ms не подтверждён ни своим кадром, ни совпадением, а снимок другой, —
//! расхождение: ядру снимок, быстрый стакан пересинхронизируется (resync). Сопоставить кадры двух каналов по номеру
//! нельзя: связь seqNum mbp.refresh и seqNum mbp.$levels документация не описывает.
//! Правило «не старее отданного» (как решение 109): ядру не отдаётся стакан со временем биржи (ts кадра) меньше,
//! чем у уже отданного; нет времени — сравнения нет.
//! Кто вызывает: ws_conn.rs, ws.rs (снимки), fast_pair.rs, fast_conn.rs (быстрый канал), market.rs (сброс).
//! От чего зависит: book.rs (Books, Alive), book_fast.rs (same_top), fast_log.rs, fast_resync.rs, core/types.
//! Настройки: нет своих (venue.book_stale_ms читает вызывающий).
//! Порядок замков: сначала pairs этого файла, потом стаканы Books (same_top) — обратного порядка нет.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::core::types::OrderBook;

use super::book::{Alive, Books};
use super::fast_log::FastLog;
use super::fast_resync::Resync;

/// Состояние выбора. Дешёво клонируется (общее для соединений снимков и быстрого канала).
#[derive(Clone, Default)]
pub struct Mode {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    pairs: Mutex<HashMap<String, Pair>>,
    /// Каналы пересинхронизации соединений быстрого канала (fast_resync.rs).
    resync: Resync,
    /// Уровни быстрого канала; 0 — быстрой ветки нет (только снимки, как до решения 422).
    levels: AtomicU64,
    log: FastLog,
}

/// Пара.
#[derive(Default)]
struct Pair {
    /// Ядру отдаётся быстрый стакан.
    fast: bool,
    /// Время биржи (мс) последнего отданного ядру стакана.
    last_ms: Option<i64>,
    /// Наши часы (мс): последний кадр быстрого потока пары или совпадение со снимком.
    ok_ms: i64,
    /// Последний снимок mbp.refresh и живость его соединения.
    spare: Option<(OrderBook, Alive)>,
}

/// Что делать с кадром снимков.
#[derive(Debug, PartialEq, Eq)]
pub enum Snap {
    /// Отдать ядру (в память и ядру, как до решения 422).
    Give,
    /// Только время кадра пары (Books::touch).
    Touch,
    /// Ничего: ядру отдаётся быстрый стакан, или снимок старее отданного.
    Hold,
    /// Расхождение: вызывающий уводит пару на снимки (fall_back → fast_pair.rs back) и просит resync.
    Diverged,
}

/// Что делать при уходе с быстрого стакана.
pub enum Back {
    /// Пара уже на снимках.
    Nothing,
    /// Запасной снимок в память (время кадра — его received_at) и ядру.
    Use(OrderBook, Alive),
    /// Запасного снимка нет или он старее отданного — стакан пары недостоверен до нового снимка.
    Invalidate,
}

/// Сейчас, unix мс.
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Время биржи стакана (мс). Ошибок нет.
fn ts(b: &OrderBook) -> Option<i64> {
    b.exchange_time.map(|t| t.timestamp_millis())
}

/// Старее ли ts уже отданного (нет одного из времён — нет). Ошибок нет.
fn older(ts: Option<i64>, last: Option<i64>) -> bool {
    matches!((ts, last), (Some(t), Some(l)) if t < l)
}

impl Mode {
    /// Есть ли быстрая ветка (уровни канала заданы соединением быстрого канала). Ошибок нет.
    pub fn enabled(&self) -> bool {
        self.inner.levels.load(Ordering::Relaxed) > 0
    }

    /// Уровни быстрого канала (0 — ветки нет). Ошибок нет.
    pub fn set_levels(&self, n: u64) {
        self.inner.levels.store(n, Ordering::Relaxed);
    }

    /// Счёт и журнал переключений. Ошибок нет.
    pub fn log(&self) -> &FastLog {
        &self.inner.log
    }

    /// Новая подписка (market.rs): все пары — на снимки, каналы пересинхронизации прежних соединений забыты; время
    /// последнего отданного остаётся. Ошибок нет.
    pub fn reset_all(&self) {
        let mut m = self.inner.pairs.lock().unwrap_or_else(|p| p.into_inner());
        m.values_mut().for_each(|p| p.fast = false);
        drop(m);
        self.inner.resync.clear();
    }

    /// Каналы пересинхронизации (fast_resync.rs). Ошибок нет.
    pub fn resyncs(&self) -> &Resync {
        &self.inner.resync
    }

    /// Кадр снимков пары (только при enabled). Принимает: стаканы, тикер, новый снимок (None — тот же seqNum, решение
    /// 109: сравнивается хранимый), живость соединения снимков, venue.depth_levels, чтение venue.book_stale_ms (мс;
    /// None — не задана: расхождения по времени нет). Возвращает: Snap. Ошибок нет.
    pub fn on_snapshot(
        &self,
        books: &Books,
        ticker: &str,
        frame: Option<&OrderBook>,
        alive: &Alive,
        depth_levels: usize,
        stale_ms: impl FnOnce() -> Option<i64>,
    ) -> Snap {
        let mut m = self.inner.pairs.lock().unwrap_or_else(|p| p.into_inner());
        let p = m.entry(ticker.to_string()).or_default();
        if let Some(f) = frame {
            p.spare = Some((f.clone(), alive.clone()));
        }
        let t = match &p.spare {
            None => return Snap::Hold,
            Some((s, _)) => ts(s),
        };
        if !p.fast {
            return match frame {
                None => Snap::Touch,
                Some(_) if older(t, p.last_ms) => Snap::Hold,
                Some(_) => {
                    p.last_ms = t.or(p.last_ms);
                    Snap::Give
                }
            };
        }
        let n = depth_levels.min(self.inner.levels.load(Ordering::Relaxed) as usize);
        let now = now_ms();
        let same = p
            .spare
            .as_ref()
            .is_some_and(|(s, _)| books.same_top(ticker, &s.bids, &s.asks, n));
        if same {
            p.ok_ms = now;
            return Snap::Touch;
        }
        match stale_ms() {
            Some(s) if now - p.ok_ms > s => Snap::Diverged,
            _ => Snap::Hold,
        }
    }

    /// Быстрый стакан пары изменился и синхронизирован. Принимает: тикер, время биржи последнего изменения.
    /// Возвращает: true — отдать ядру (пара на быстром; возврат с снимков — только при известном времени, не старее
    /// отданного). Ошибок нет.
    pub fn on_fast(&self, ticker: &str, t: Option<i64>) -> bool {
        let mut m = self.inner.pairs.lock().unwrap_or_else(|p| p.into_inner());
        let p = m.entry(ticker.to_string()).or_default();
        p.ok_ms = now_ms();
        if p.fast {
            p.last_ms = t.max(p.last_ms);
            return true;
        }
        if t.is_none() || older(t, p.last_ms) {
            return false;
        }
        p.fast = true;
        p.last_ms = t;
        drop(m);
        self.inner.log.to_fast();
        true
    }

    /// Кадр быстрого потока пары без изменений (цепочка продолжена). Возвращает: true — пара на быстром (тронуть время
    /// кадра пары). Ошибок нет.
    pub fn fast_alive(&self, ticker: &str) -> bool {
        let mut m = self.inner.pairs.lock().unwrap_or_else(|p| p.into_inner());
        let p = m.entry(ticker.to_string()).or_default();
        p.ok_ms = now_ms();
        p.fast
    }

    /// Уход пары с быстрого стакана (разрыв, ошибка, конец сессии). Принимает: тикер, причину. Возвращает: Back.
    /// Ошибок нет.
    pub fn fall_back(&self, ticker: &str, why: &str) -> Back {
        let mut m = self.inner.pairs.lock().unwrap_or_else(|p| p.into_inner());
        let Some(p) = m.get_mut(ticker).filter(|p| p.fast) else {
            return Back::Nothing;
        };
        p.fast = false;
        let back = match &p.spare {
            Some((b, a)) if !older(ts(b), p.last_ms) => {
                p.last_ms = ts(b).or(p.last_ms);
                Back::Use(b.clone(), a.clone())
            }
            _ => Back::Invalidate,
        };
        drop(m);
        self.inner.log.to_snap(&format!("{ticker}: {why}"));
        back
    }

    /// Соединение снимков пары оборвалось или кадр плохой: запасного снимка больше нет. Возвращает: true — пара на
    /// быстром (её стакан не трогать). Ошибок нет.
    pub fn snapshot_lost(&self, ticker: &str) -> bool {
        let mut m = self.inner.pairs.lock().unwrap_or_else(|p| p.into_inner());
        m.get_mut(ticker).is_some_and(|p| {
            p.spare = None;
            p.fast
        })
    }
}
