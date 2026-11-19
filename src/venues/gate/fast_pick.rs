//! Файл: venues/gate/fast_pick.rs
//! Что делает: решение 421 — какой стакан пары Gate отдавать ядру: быстрый (локальный стакан потока spot.obu,
//! fast_book.rs), пока он синхронизирован, иначе — снимок spot.order_book (страховка, слушается параллельно). Разрыв
//! номеров, неразборчивый кадр, отказ подписки → сразу последний снимок (если он не старее уже отданного), иначе стакан
//! пары снимается; быстрый стакан пересинхронизируется (очередь отписки/подписки потока, темп — venue.ws_subs_per_sec
//! в ws_conn.rs). После нового полного снимка — снова быстрый. Чистая логика одного соединения (без сети и журнала).
//! Кто вызывает: ws_conn.rs, fast_io.rs, тесты.
//! От чего зависит: fast_book.rs, obu.rs (Frame), depth.rs (to_book), core/types.
//! Настройки: нет (уровни потока и глубину передаёт ws_conn.rs из Cfg).
//!
//! Правило «не старее отданного» (смена потока — только по t строго больше) и сверка верха снимка с быстрым стаканом
//! (два снимка подряд новее быстрого и не равны ему, быстрый между ними не менялся → разрыв; решение 421, ревью) —
//! fast_rule.rs.

use std::collections::{HashMap, VecDeque};

use chrono::{TimeZone, Utc};

use crate::core::types::{Asset, OrderBook};

use super::depth;
use super::fast_book::{Applied, FastBook};
use super::fast_rule::{Given, Src, allow, diverged, ms};
use super::obu::Frame;

/// Что сделать со стаканом пары у ядра.
#[derive(Debug, Clone)]
pub enum Give {
    /// Отдать этот стакан.
    Book(OrderBook),
    /// Снять стакан пары (недостоверен): быстрый разорван, а снимка новее отданного нет.
    Drop,
    /// Ничего.
    Nothing,
}

/// Состояние пары.
#[derive(Debug, Default)]
struct Pair {
    lb: FastBook,
    /// Последний отданный ядру: источник и время биржи t (мс).
    given: Given,
    /// Последний снимок, не отданный ядру (пока ядро на быстром стакане), — для немедленного возврата на снимки.
    held: Option<OrderBook>,
    /// Пара в очереди пересинхронизации.
    queued: bool,
    /// Подписка spot.obu отклонена биржей — до нового подключения только снимки.
    off: bool,
    /// u быстрого стакана при прошлом несовпавшем снимке (fast_rule.rs diverged; решение 421, ревью).
    miss: Option<i64>,
}

/// Быстрые стаканы одного соединения.
#[derive(Debug)]
pub struct Fast {
    /// Уровни потока spot.obu (venue.obu_level); None — быстрого потока нет, только снимки (как до решения 421).
    level: Option<u64>,
    /// Сколько уровней отдавать ядру: меньшее из venue.depth_levels и уровней потока.
    depth: usize,
    pairs: HashMap<String, Pair>,
    /// Очередь пересинхронизации (символы пар).
    queue: VecDeque<String>,
}

impl Fast {
    /// Принимает: уровни потока (None — только снимки), venue.depth_levels. Ошибок нет.
    pub fn new(level: Option<u64>, depth_levels: u64) -> Self {
        let depth = level.map_or(depth_levels, |l| l.min(depth_levels)) as usize;
        Self {
            level,
            depth,
            pairs: HashMap::new(),
            queue: VecDeque::new(),
        }
    }

    /// Уровни потока spot.obu; None — быстрого потока нет. Ошибок нет.
    pub fn level(&self) -> Option<u64> {
        self.level
    }

    /// Что делает: снимок spot.order_book (уже прошедший отсев решения 109). Принимает: символ, стакан. Возвращает:
    /// стакан ядру (None — придержан: ядро на быстром стакане, или старее отданного) и причину разрыва (Some — верх
    /// снимка второй раз подряд разошёлся с быстрым стаканом: быстрый сброшен, пара в очереди пересинхронизации, ядру —
    /// этот снимок; fast_rule.rs diverged). Ошибок нет.
    pub fn snapshot(
        &mut self,
        symbol: &str,
        book: OrderBook,
    ) -> (Option<OrderBook>, Option<String>) {
        if self.level.is_none() {
            return (Some(book), None);
        }
        let n = self.depth;
        let p = self.pairs.entry(symbol.to_string()).or_default();
        let on_fast = p.lb.synced() && matches!(p.given, Some((Src::Fast, _)));
        let gap = on_fast && diverged(&p.lb, &book, n, &mut p.miss);
        if !gap && (on_fast || !allow(&p.given, Src::Snap, ms(&book))) {
            p.held = Some(book);
            return (None, None);
        }
        p.given = Some((Src::Snap, ms(&book)));
        p.held = None;
        if !gap {
            return (Some(book), None);
        }
        self.resync(symbol);
        let why = "два снимка spot.order_book подряд новее быстрого стакана и не совпали с ним, а он между ними не менялся";
        (Some(book), Some(why.to_string()))
    }

    /// Что делает: кадр spot.obu пары. Принимает: символ, монету, кадр. Возвращает: что отдать ядру и причину разрыва
    /// (Some — разрыв: стакан сброшен, пара в очереди пересинхронизации). Ошибок нет.
    pub fn frame(&mut self, symbol: &str, asset: &Asset, f: &Frame) -> (Give, Option<String>) {
        let n = self.depth;
        let p = self.pairs.entry(symbol.to_string()).or_default();
        if p.off {
            return (Give::Nothing, None);
        }
        match p.lb.apply(f) {
            Applied::Waiting => (Give::Nothing, None),
            Applied::Gap(why) => (self.lost(symbol), Some(why)),
            Applied::Changed => {
                let t = p.lb.time();
                if !allow(&p.given, Src::Fast, t) {
                    return (Give::Nothing, None);
                }
                p.given = Some((Src::Fast, t));
                let (bids, asks) = p.lb.top(n);
                let time = t.and_then(|m| Utc.timestamp_millis_opt(m).single());
                (
                    Give::Book(depth::to_book(asset.clone(), bids, asks, time)),
                    None,
                )
            }
        }
    }

    /// Что делает: быстрый стакан пары недостоверен (разрыв, неразборчивый кадр): сброс, очередь пересинхронизации и
    /// немедленный возврат на снимки. Принимает: символ. Возвращает: что отдать ядру. Ошибок нет.
    pub fn lost(&mut self, symbol: &str) -> Give {
        self.resync(symbol);
        self.fallback(symbol)
    }

    /// Что делает: сторож пар (решение 202) переподписывает пару — быстрый стакан тоже заново (сброс и очередь); стакан у
    /// ядра не трогается (как при переподписке снимков). Принимает: символ. Ошибок нет.
    pub fn resync(&mut self, symbol: &str) {
        if self.level.is_none() {
            return;
        }
        let p = self.pairs.entry(symbol.to_string()).or_default();
        let _ = p.lb.reset(String::new());
        p.miss = None;
        if !p.queued && !p.off {
            p.queued = true;
            self.queue.push_back(symbol.to_string());
        }
    }

    /// Что делает: подписка spot.obu пары отклонена — до нового подключения только снимки. Принимает: символ.
    /// Возвращает: что отдать ядру. Ошибок нет.
    pub fn refused(&mut self, symbol: &str) -> Give {
        let p = self.pairs.entry(symbol.to_string()).or_default();
        p.off = true;
        let _ = p.lb.reset(String::new());
        self.fallback(symbol)
    }

    /// Следующая пара для пересинхронизации (отписка и подписка потока). None — очередь пуста. Ошибок нет.
    pub fn next_resync(&mut self) -> Option<String> {
        while let Some(s) = self.queue.pop_front() {
            if let Some(p) = self.pairs.get_mut(&s) {
                p.queued = false;
                if !p.off {
                    return Some(s);
                }
            }
        }
        None
    }

    /// Ядро сейчас на быстром стакане пары (он синхронизирован и отдан последним) — сбой снимков его не снимает.
    /// Ошибок нет.
    pub fn on_fast(&self, symbol: &str) -> bool {
        self.pairs
            .get(symbol)
            .is_some_and(|p| p.lb.synced() && matches!(p.given, Some((Src::Fast, _))))
    }

    /// Есть ли пары в очереди пересинхронизации. Ошибок нет.
    pub fn pending(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Возврат на снимки: ядро было на быстром — последний придержанный снимок, если он новее отданного, иначе снять
    /// стакан; ядро уже на снимках — ничего. Ошибок нет.
    fn fallback(&mut self, symbol: &str) -> Give {
        let Some(p) = self.pairs.get_mut(symbol) else {
            return Give::Nothing;
        };
        if !matches!(p.given, Some((Src::Fast, _))) {
            return Give::Nothing;
        }
        match p.held.take() {
            Some(h) if allow(&p.given, Src::Snap, ms(&h)) => {
                p.given = Some((Src::Snap, ms(&h)));
                Give::Book(h)
            }
            _ => Give::Drop,
        }
    }
}
