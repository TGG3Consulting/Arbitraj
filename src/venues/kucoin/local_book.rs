//! Файл: venues/kucoin/local_book.rs
//! Что делает: решение 423 — локальный стакан одной пары KuCoin по процедуре «Orderbook - Increment» (калибровка):
//! изменения /market/level2 копятся, пока нет снимка REST Get Full OrderBook; снимок кладётся целиком, затем
//! проигрываются накопленные изменения; кадр принимается, если sequenceStart(new) <= sequenceEnd(old)+1 и
//! sequenceEnd(new) > sequenceEnd(old); иначе — разрыв (стакан недостоверен до нового снимка). Отдаёт верхние уровни
//! в формате ядра и время биржи последнего применённого изменения.
//! Кто вызывает: ws_fast.rs (кадр изменений, итог снимка REST), тесты.
//! От чего зависит: core/types (PriceLevel), gateway/feed_health.rs (BookPrint).
//! Настройки: нет.
//!
//! Официально (https://www.kucoin.com/docs-new/3470068w0, Orderbook - Increment, «Calibration procedure»):
//! «After receiving the websocket Level 2 data flow, cache the data», «Initiate a Level 2 request to» Get Full
//! OrderBook, «Playback the cached Level 2 data flow», «sequenceStart(new) < = sequenceEnd+1(old) and sequenceEnd(new) >
//! sequenceEnd(old)»; «If the price is 0, ignore the messages and update the sequence»; «If the size=0, update the
//! sequence and remove the price of which the size is 0 out of level 2»; «For other cases, please update the price».
//! Пример той же страницы (снимок sequence 16, кадр sequenceStart 15 — sequenceEnd 19): запись ["3988.59","3","16"] и
//! ["3988.62","8","15"] пропускаются (номер записи ≤ номера снимка), ["3988.61","0","19"] убирает уровень,
//! ["3988.50","44","18"] меняет количество. Номер записи «does not serve as a basis for judging message continuity» —
//! непрерывность только по sequenceStart / sequenceEnd кадра; номер записи — только пропуск уже учтённого (пример).

use std::collections::BTreeMap;

use rust_decimal::Decimal;

use crate::core::types::PriceLevel;
use crate::gateway::feed_health::BookPrint;

/// Одна запись changes: цена, количество, номер последнего изменения этой цены.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub price: Decimal,
    pub size: Decimal,
    pub seq: i64,
}

/// Кадр изменений /market/level2 в числах ядра.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub start: i64,
    pub end: i64,
    /// Время биржи кадра, мс («time … milliseconds»).
    pub time: i64,
    pub bids: Vec<Change>,
    pub asks: Vec<Change>,
}

/// Что сделал кадр или снимок.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// Стакан изменился.
    Changed,
    /// Кадр уже учтён (sequenceEnd ≤ номер стакана).
    Repeat,
    /// Снимка ещё нет — кадр отложен.
    Waiting,
    /// Разрыв: стакан сброшен до нового снимка (причина).
    Gap(String),
}

/// Стакан пары.
#[derive(Debug, Default, Clone)]
pub struct LocalBook {
    bids: BTreeMap<Decimal, Decimal>,
    asks: BTreeMap<Decimal, Decimal>,
    /// sequenceEnd последнего учтённого кадра или sequence снимка; None — снимка нет.
    seq: Option<i64>,
    /// Время биржи последнего применённого кадра (мс); None — после снимка изменений ещё не было.
    time: Option<i64>,
    /// Кадры, пришедшие до снимка.
    buffer: Vec<Update>,
}

/// Записи стороны поверх стакана с номером seq (правила — в шапке). Ошибок нет.
fn put(side: &mut BTreeMap<Decimal, Decimal>, changes: &[Change], seq: i64) {
    for c in changes {
        if c.seq <= seq || c.price.is_zero() {
            continue;
        }
        if c.size.is_zero() {
            side.remove(&c.price);
        } else {
            side.insert(c.price, c.size);
        }
    }
}

impl LocalBook {
    /// Синхронизирован ли стакан (снимок есть). Ошибок нет.
    pub fn synced(&self) -> bool {
        self.seq.is_some()
    }

    /// Номер стакана (sequence снимка или sequenceEnd последнего кадра); None — снимка нет. Ошибок нет.
    pub fn version(&self) -> Option<i64> {
        self.seq
    }

    /// Время биржи последнего применённого кадра, мс; None — снимка нет или изменений после снимка не было (время
    /// ответа REST с временем кадров не сравнивается — их связь в документации не описана). Ошибок нет.
    pub fn time(&self) -> Option<i64> {
        self.seq.and(self.time)
    }

    /// Что делает: кадр изменений. Принимает: кадр. Возвращает: Applied. Ошибок нет: разрыв — Gap (стакан сброшен).
    pub fn apply(&mut self, u: Update) -> Applied {
        let Some(seq) = self.seq else {
            self.buffer.push(u);
            return Applied::Waiting;
        };
        if u.end <= seq {
            return Applied::Repeat;
        }
        if u.start > seq + 1 {
            return self.reset(format!(
                "sequenceStart {} не продолжает номер стакана {seq} (sequenceEnd {})",
                u.start, u.end
            ));
        }
        put(&mut self.bids, &u.bids, seq);
        put(&mut self.asks, &u.asks, seq);
        self.seq = Some(u.end);
        self.time = Some(u.time);
        Applied::Changed
    }

    /// Что делает: снимок REST: уровни целиком, затем накопленные кадры по правилам. Принимает: sequence снимка,
    /// уровни (нулевые — без уровня). Возвращает: Changed или Gap (накопленные не продолжают снимок). Ошибок нет.
    pub fn snapshot(&mut self, seq: i64, bids: &[PriceLevel], asks: &[PriceLevel]) -> Applied {
        self.bids.clear();
        self.asks.clear();
        for (side, levels) in [(&mut self.bids, bids), (&mut self.asks, asks)] {
            for l in levels.iter().filter(|l| l.qty > Decimal::ZERO) {
                side.insert(l.price, l.qty);
            }
        }
        self.seq = Some(seq);
        self.time = None;
        for u in std::mem::take(&mut self.buffer) {
            if let Applied::Gap(why) = self.apply(u) {
                return Applied::Gap(why);
            }
        }
        Applied::Changed
    }

    /// Сбрасывает стакан (недостоверен до нового снимка). Возвращает: Gap с причиной. Ошибок нет.
    pub fn reset(&mut self, why: String) -> Applied {
        self.bids.clear();
        self.asks.clear();
        self.seq = None;
        self.time = None;
        self.buffer.clear();
        Applied::Gap(why)
    }

    /// Снимок не запрошен (пауза после разрыва) — накопленные кадры не нужны: снимок, запрошенный позже, их
    /// перекроет (кадры до запроса — sequenceEnd ≤ sequence снимка). Ошибок нет.
    pub fn clear_buffer(&mut self) {
        self.buffer.clear();
    }

    /// Верхние уровни: bids по убыванию цены, asks по возрастанию, не больше depth на сторону. Ошибок нет.
    pub fn top(&self, depth: usize) -> (Vec<PriceLevel>, Vec<PriceLevel>) {
        let lv = |(p, q): (&Decimal, &Decimal)| PriceLevel { price: *p, qty: *q };
        (
            self.bids.iter().rev().take(depth).map(lv).collect(),
            self.asks.iter().take(depth).map(lv).collect(),
        )
    }

    /// Отпечаток верхних depth уровней (для сверки со снимком — fast_check.rs). Ошибок нет.
    pub fn print(&self, depth: usize) -> BookPrint {
        let (bids, asks) = self.top(depth);
        BookPrint { bids, asks }
    }
}
