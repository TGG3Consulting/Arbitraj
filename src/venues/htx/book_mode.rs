//! Файл: venues/htx/book_mode.rs
//! Что делает: решение 422 — какой стакан пары HTX отдаётся ядру: быстрый (локальный стакан канала /feed
//! market.$symbol.mbp.$levels, fast_pair.rs), пока он синхронизирован, иначе — снимок канала /ws mbp.refresh
//! (страховка, ws_conn.rs). Последний снимок пары хранится всегда (запасной). Разрыв, конец сессии быстрого канала —
//! сразу запасной снимок (fall_back). Сверка по содержимому (book_rule.rs diverged; решение 422, ревью): снимок,
//! совпавший с быстрым стаканом в первых min(venue.depth_levels, уровни быстрого канала) уровнях, подтверждает его
//! (время кадра пары); два снимка подряд новее быстрого и не равны ему, а быстрый между ними не менялся, —
//! расхождение: ядру снимок, быстрый пересинхронизируется (resync), возврат на быстрый — только после нового снимка
//! req (resynced). Сопоставить кадры двух каналов по номеру нельзя: связь seqNum mbp.refresh и seqNum mbp.$levels
//! документация не описывает.
//! Правило «не старее отданного» (как решение 109): ядру не отдаётся стакан со временем биржи (ts кадра) меньше,
//! чем у уже отданного; нет времени — сравнения нет.
//! Порядок отдачи (решение 422, ревью; как ревью 427/429): решение «что отдать» и сама отдача (запись в память и канал
//! ядра — действие вызывающего, без ожиданий) идут под одним замком pairs — соединения /ws и /feed не отдают ядру
//! стаканы не по порядку.
//! Кто вызывает: ws_conn.rs, ws.rs (снимки, через snap_out.rs), fast_pair.rs, fast_ws.rs (быстрый канал), market.rs.
//! От чего зависит: book.rs (Books, Alive), book_fast.rs (same_top), book_rule.rs, fast_log.rs, fast_resync.rs.
//! Настройки: нет своих.
//! Порядок замков: pairs этого файла → стаканы Books (same_top, put) → набор канала ядра; обратного порядка нет.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::core::types::OrderBook;

use super::book::{Alive, Books};
use super::book_rule::{Pair, diverged, leave, older, ts};
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

/// Что делать с кадром снимков.
#[derive(Debug, PartialEq, Eq)]
pub enum Snap {
    /// Отдать ядру (в память и ядру, как до решения 422).
    Give,
    /// Только время кадра пары (Books::touch).
    Touch,
    /// Ничего: ядру отдаётся быстрый стакан, или снимок старее отданного.
    Hold,
    /// Расхождение: пара уже ушла на снимки (Back — что отдать); вызывающий просит resync.
    Diverged,
}

/// Что делать при уходе с быстрого стакана.
#[derive(Debug)]
pub enum Back {
    /// Пара уже на снимках.
    Nothing,
    /// Запасной снимок в память (время кадра — его received_at) и ядру.
    Use(OrderBook, Alive),
    /// Запасного снимка нет или он старее отданного — стакан пары недостоверен до нового снимка.
    Invalidate,
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
        m.values_mut().for_each(|p| {
            p.fast = false;
            p.miss = None;
            p.blocked = false;
        });
        drop(m);
        self.inner.resync.clear();
    }

    /// Каналы пересинхронизации (fast_resync.rs). Ошибок нет.
    pub fn resyncs(&self) -> &Resync {
        &self.inner.resync
    }

    /// Кадр снимков пары (только при enabled). Принимает: стаканы, тикер, новый снимок (None — тот же seqNum, решение
    /// 109: сравнивается хранимый, в счёт «подряд» не идёт), живость соединения снимков, venue.depth_levels, действие
    /// (выбор; при Diverged — что отдать) — выполняется под замком пар. Возвращает: выбор и итог действия. Ошибок нет.
    pub fn on_snapshot<R>(
        &self,
        books: &Books,
        ticker: &str,
        frame: Option<&OrderBook>,
        alive: &Alive,
        depth_levels: usize,
        act: impl FnOnce(&Snap, Back) -> R,
    ) -> (Snap, R) {
        let mut m = self.inner.pairs.lock().unwrap_or_else(|p| p.into_inner());
        let p = m.entry(ticker.to_string()).or_default();
        if let Some(f) = frame {
            p.spare = Some((f.clone(), alive.clone()));
        }
        let spare_ts = p.spare.as_ref().map(|(s, _)| ts(s));
        let snap = match spare_ts {
            None => Snap::Hold,
            Some(t) if !p.fast => match frame {
                None => Snap::Touch,
                Some(_) if older(t, p.last_ms) => Snap::Hold,
                Some(_) => {
                    p.last_ms = t.or(p.last_ms);
                    Snap::Give
                }
            },
            Some(_) => {
                let n = depth_levels.min(self.inner.levels.load(Ordering::Relaxed) as usize);
                let same = p
                    .spare
                    .as_ref()
                    .is_some_and(|(s, _)| books.same_top(ticker, &s.bids, &s.asks, n));
                if same {
                    p.miss = None;
                    Snap::Touch
                } else if frame.is_some() && diverged(p, false) {
                    Snap::Diverged
                } else {
                    Snap::Hold
                }
            }
        };
        let back = if snap == Snap::Diverged {
            p.blocked = true;
            let b = leave(p);
            let why =
                format!("{ticker}: два снимка mbp.refresh подряд не совпали с быстрым стаканом");
            self.inner.log.to_snap(&why);
            b
        } else {
            Back::Nothing
        };
        let r = act(&snap, back);
        (snap, r)
    }

    /// Быстрый стакан пары изменился и синхронизирован. Принимает: тикер, время биржи последнего изменения, отдачу
    /// ядру (выполняется под замком пар). Возвращает: Some — отдано (пара на быстром; возврат со снимков — только при
    /// известном времени, не старее отданного и без объявленного расхождения). Ошибок нет.
    pub fn on_fast<R>(&self, ticker: &str, t: Option<i64>, give: impl FnOnce() -> R) -> Option<R> {
        let mut m = self.inner.pairs.lock().unwrap_or_else(|p| p.into_inner());
        let p = m.entry(ticker.to_string()).or_default();
        if !p.fast {
            if p.blocked || t.is_none() || older(t, p.last_ms) {
                return None;
            }
            p.fast = true;
            p.miss = None;
            self.inner.log.to_fast();
        }
        p.last_ms = t.max(p.last_ms);
        p.ver += 1;
        Some(give())
    }

    /// Пришёл снимок req быстрого канала пары (после сброса): объявленное расхождение снято. Ошибок нет.
    pub fn resynced(&self, ticker: &str) {
        let mut m = self.inner.pairs.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(p) = m.get_mut(ticker) {
            p.blocked = false;
        }
    }

    /// Кадр быстрого потока пары без изменений (цепочка продолжена). Возвращает: true — пара на быстром (тронуть время
    /// кадра пары). Ошибок нет.
    pub fn fast_alive(&self, ticker: &str) -> bool {
        let m = self.inner.pairs.lock().unwrap_or_else(|p| p.into_inner());
        m.get(ticker).is_some_and(|p| p.fast)
    }

    /// Уход пары с быстрого стакана (разрыв, ошибка, конец сессии). Принимает: тикер, причину, отдачу (выполняется под
    /// замком пар). Возвращает: итог отдачи. Ошибок нет.
    pub fn fall_back<R>(&self, ticker: &str, why: &str, act: impl FnOnce(Back) -> R) -> R {
        let mut m = self.inner.pairs.lock().unwrap_or_else(|p| p.into_inner());
        let back = match m.get_mut(ticker).filter(|p| p.fast) {
            None => Back::Nothing,
            Some(p) => {
                let b = leave(p);
                self.inner.log.to_snap(&format!("{ticker}: {why}"));
                b
            }
        };
        act(back)
    }

    /// Соединение снимков пары оборвалось или кадр плохой: запасного снимка больше нет; пара не на быстром — действие
    /// «стакан недостоверен» под замком пар (быстрый канал не вернёт пару на быстрый между проверкой и снятием).
    /// Принимает: тикер, действие. Ошибок нет.
    pub fn snapshot_lost(&self, ticker: &str, drop_book: impl FnOnce()) {
        let mut m = self.inner.pairs.lock().unwrap_or_else(|p| p.into_inner());
        let fast = m.get_mut(ticker).is_some_and(|p| {
            p.spare = None;
            p.miss = None;
            p.fast
        });
        if !fast {
            drop_book();
        }
    }
}
