//! Файл: venues/binance/sbe_book.rs
//! Что делает: решение 425 — локальный стакан одной пары Binance по официальной процедуре «How to manage a local order
//! book correctly»: изменения копятся с момента, когда снимок REST реально запрашивается (begin), снимок с lastUpdateId
//! меньше U первого отложенного — запросить заново (пункт 4), отложенные с u ≤ lastUpdateId — отбросить (пункт 5),
//! дальше U > номер стакана + 1 — пропуск, стакан сбрасывается до нового снимка. Количество — абсолютное, 0 — убрать
//! уровень. Отдаёт верхние уровни в формате ядра.
//! Кто вызывает: sbe_pair.rs (кадр изменений SBE и снимок из sbe_snap.rs), тесты.
//! От чего зависит: core/types (PriceLevel).
//! Настройки: нет.
//!
//! Официально (github.com/binance/binance-spot-api-docs, web-socket-streams.md «How to manage a local order book
//! correctly»; sbe-market-data-streams.md «Diff. Depth Streams» ссылается на ту же процедуру): «Buffer the events
//! received from the stream. Note the U of the first event you received»; «If the lastUpdateId from the snapshot is
//! strictly less than the U from step 2, go back to step 3»; «discard any event where u is <= lastUpdateId of the
//! snapshot»; «If the event last update ID (u) is less than the update ID of your local order book, ignore the event»;
//! «If the event first update ID (U) is greater than the update ID of your local order book + 1, you have missed some
//! events. Discard your local order book and restart the process from the beginning»; «If the quantity is zero, remove
//! the price level»; «Set the order book update ID to the last update ID (u) in the processed event».
//! У SBE U и u — поля firstBookUpdateId и lastBookUpdateId сообщения DepthDiffStreamEvent (sbe.rs).
//! Отложенные кадры — только с начала запроса снимка (begin), а не с подписки: кадр, полученный до запроса, отбросился
//! бы по пункту 5; если же отбросился нужный — пункт 4 (lastUpdateId < U) просит снимок заново. Так очередь не растёт,
//! пока пара ждёт своей очереди снимков.

use std::collections::BTreeMap;

use rust_decimal::Decimal;

use crate::core::types::PriceLevel;

/// Изменения одного кадра в числах ядра.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    /// U.
    pub first: i64,
    /// u.
    pub last: i64,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
}

/// Что сделал кадр или снимок.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// Стакан изменился — отдать ядру.
    Changed,
    /// Кадр уже учтён (u ≤ номер стакана) — ничего.
    Old,
    /// Снимка нет и он не запрашивается — кадр не нужен; пора просить снимок.
    Idle,
    /// Снимок запрашивается — кадр отложен.
    Buffered,
    /// Пункт 4: lastUpdateId снимка меньше U первого отложенного — снимок заново (отложенные сохранены).
    Retry,
    /// Пропуск номеров: стакан недостоверен до нового снимка (причина).
    Gap(String),
}

/// Стакан пары.
#[derive(Debug, Default, Clone)]
pub struct LocalBook {
    bids: BTreeMap<Decimal, Decimal>,
    asks: BTreeMap<Decimal, Decimal>,
    /// Номер стакана (lastUpdateId снимка или u последнего применённого); None — снимка нет.
    id: Option<i64>,
    /// Снимок запрашивается — кадры копятся.
    buffering: bool,
    buffer: Vec<Update>,
}

/// Уровни стороны: новое количество, 0 — убрать. Ошибок нет.
fn put(side: &mut BTreeMap<Decimal, Decimal>, levels: &[PriceLevel]) {
    for l in levels {
        if l.qty.is_zero() {
            side.remove(&l.price);
        } else {
            side.insert(l.price, l.qty);
        }
    }
}

impl LocalBook {
    /// Синхронизирован ли стакан (снимок применён). Ошибок нет.
    pub fn synced(&self) -> bool {
        self.id.is_some()
    }

    /// Номер стакана (lastUpdateId снимка или u последнего изменения); None — снимка нет. Ошибок нет.
    pub fn version(&self) -> Option<i64> {
        self.id
    }

    /// Что делает: снимок REST сейчас будет запрошен — начинать копить кадры (пункт 2). Уже копятся (повтор по пункту
    /// 4 — «go back to step 3») — отложенные сохраняются. Ошибок нет.
    pub fn begin(&mut self) {
        if !self.buffering {
            self.buffer.clear();
            self.buffering = true;
        }
    }

    /// Что делает: кадр изменений. Принимает: изменения. Возвращает: Applied. Ошибок нет: пропуск — Gap (стакан
    /// сброшен).
    pub fn apply(&mut self, u: Update) -> Applied {
        let Some(id) = self.id else {
            if self.buffering {
                self.buffer.push(u);
                return Applied::Buffered;
            }
            return Applied::Idle;
        };
        // Пункт 5 (u ≤ lastUpdateId — отбросить) и правило «u меньше номера стакана — пропустить».
        if u.last <= id {
            return Applied::Old;
        }
        if u.first > id + 1 {
            return self.reset(format!(
                "U {} больше номера стакана {id} + 1 (u {}) — пропущены изменения",
                u.first, u.last
            ));
        }
        put(&mut self.bids, &u.bids);
        put(&mut self.asks, &u.asks);
        self.id = Some(u.last);
        Applied::Changed
    }

    /// Что делает: снимок REST (пункты 4–7). Принимает: lastUpdateId, уровни снимка. Возвращает: Changed, Retry
    /// (lastUpdateId < U первого отложенного) или Gap (отложенные не продолжают снимок). Ошибок нет.
    pub fn snapshot(&mut self, last_id: i64, bids: &[PriceLevel], asks: &[PriceLevel]) -> Applied {
        if let Some(first) = self.buffer.first()
            && last_id < first.first
        {
            return Applied::Retry;
        }
        self.bids.clear();
        self.asks.clear();
        put(&mut self.bids, bids);
        put(&mut self.asks, asks);
        self.id = Some(last_id);
        self.buffering = false;
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
        self.id = None;
        self.buffering = false;
        self.buffer.clear();
        Applied::Gap(why)
    }

    /// Верхние уровни: bids по убыванию цены, asks по возрастанию, не больше depth на сторону. Ошибок нет.
    pub fn top(&self, depth: usize) -> (Vec<PriceLevel>, Vec<PriceLevel>) {
        let lv = |(p, q): (&Decimal, &Decimal)| PriceLevel { price: *p, qty: *q };
        (
            self.bids.iter().rev().take(depth).map(lv).collect(),
            self.asks.iter().take(depth).map(lv).collect(),
        )
    }
}
