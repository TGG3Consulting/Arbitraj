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
//! «Никогда не старее отданного» (общая постановка решений 420–424, как решение 109): внутри одного потока порядок
//! держат номера (снимки — lastUpdateId в ws_conn.rs, решение 109; быстрый — U = номер + 1). При смене потока номера
//! сравнить нельзя: совпадает ли шкала U/u spot.obu с lastUpdateId spot.order_book, документация не говорит и живой
//! записи spot.obu нет (вопрос в docs/TASKS.md) — поэтому при смене потока стакан отдаётся, только если его время биржи
//! t строго больше t последнего отданного (оба — мс Gate: у spot.order_book «Order book update time in milliseconds», у
//! spot.obu «order book generation timestamp»). Нет t — смены потока нет.
//! Сверки верхних уровней снимка с быстрым стаканом нет: сопоставить кадры двух потоков по номеру или времени
//! документация не позволяет (см. выше) — не выдумываем.

use std::collections::{HashMap, VecDeque};

use chrono::{TimeZone, Utc};

use crate::core::types::{Asset, OrderBook};

use super::depth;
use super::fast_book::{Applied, FastBook};
use super::obu::Frame;

/// Источник отданного ядру стакана.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Src {
    /// Быстрый поток spot.obu.
    Fast,
    /// Снимки spot.order_book (страховка).
    Snap,
}

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
    given: Option<(Src, Option<i64>)>,
    /// Последний снимок, не отданный ядру (пока ядро на быстром стакане), — для немедленного возврата на снимки.
    held: Option<OrderBook>,
    /// Пара в очереди пересинхронизации.
    queued: bool,
    /// Подписка spot.obu отклонена биржей — до нового подключения только снимки.
    off: bool,
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

/// Время биржи стакана, мс. Ошибок нет.
fn ms(b: &OrderBook) -> Option<i64> {
    b.exchange_time.map(|d| d.timestamp_millis())
}

/// Можно ли отдать стакан источника src со временем t после уже отданного. Ошибок нет.
fn allow(given: &Option<(Src, Option<i64>)>, src: Src, t: Option<i64>) -> bool {
    match given {
        None => true,
        Some((s, _)) if *s == src => true,
        Some((_, prev)) => matches!((prev, t), (Some(a), Some(b)) if b > *a),
    }
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
    /// Some — отдать ядру; None — придержан (ядро на быстром стакане) или старее отданного. Ошибок нет.
    pub fn snapshot(&mut self, symbol: &str, book: OrderBook) -> Option<OrderBook> {
        if self.level.is_none() {
            return Some(book);
        }
        let p = self.pairs.entry(symbol.to_string()).or_default();
        let on_fast = p.lb.synced() && matches!(p.given, Some((Src::Fast, _)));
        if on_fast || !allow(&p.given, Src::Snap, ms(&book)) {
            p.held = Some(book);
            return None;
        }
        p.given = Some((Src::Snap, ms(&book)));
        p.held = None;
        Some(book)
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
