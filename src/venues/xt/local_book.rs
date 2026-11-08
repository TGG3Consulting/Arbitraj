//! Файл: venues/xt/local_book.rs
//! Что делает: локальный стакан одной пары XT.com по инструкции «Orderbook manage»: изменения depth_update копятся,
//! пока не придёт снимок REST; изменения с i ≤ lastUpdateId снимка отбрасываются; первое применённое — с
//! fi ≤ lastUpdateId+1 ≤ i; дальше каждое fi = прежний i + 1, иначе разрыв (пересинхронизация в ws_conn.rs).
//! Количество — абсолютное, 0 — убрать уровень. Отдаёт верхние уровни в формате ядра.
//! Кто вызывает: ws_diff.rs (по кадру изменений и по снимку из snapshots.rs), ws_book.rs (номер стакана — решение
//! 213), тесты.
//! От чего зависит: core/types (PriceLevel).
//! Настройки: нет.
//!
//! Официально: https://doc.xt.com/#websocket_publicorderBook (`_websocket_public/orderBook.md`), пункты 1–9:
//! «Drop any event where i is <= lastUpdateId in the snapshot», «The first processed event should have fi <=
//! lastUpdateId+1 AND i >= lastUpdateId+1», «each new event's fi should be equal to the previous event's i+1»,
//! «Receiving an event that removes a price level that is not in your local order book can happen and is normal».

use std::collections::BTreeMap;

use rust_decimal::Decimal;

use crate::core::types::PriceLevel;

/// Изменения одного кадра в числах ядра.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub first: i64,
    pub last: i64,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
}

/// Что сделал кадр или снимок.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// Стакан изменился — отдать ядру.
    Changed,
    /// Кадр уже учтён снимком (i ≤ номер стакана) — только время кадра пары.
    Repeat,
    /// Снимка ещё нет — кадр отложен.
    Waiting,
    /// Разрыв: стакан недостоверен до нового снимка (причина).
    Gap(String),
}

/// Стакан пары.
#[derive(Debug, Default, Clone)]
pub struct LocalBook {
    bids: BTreeMap<Decimal, Decimal>,
    asks: BTreeMap<Decimal, Decimal>,
    /// Номер последнего учтённого изменения; None — снимка ещё нет.
    last: Option<i64>,
    /// После снимка ещё не применено ни одного изменения (правило «первого» кадра).
    first_pending: bool,
    /// Изменения, пришедшие до снимка.
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
    /// Синхронизирован ли стакан (снимок есть). Ошибок нет.
    pub fn synced(&self) -> bool {
        self.last.is_some()
    }

    /// Решение 213: номер стакана — lastUpdateId снимка REST или i последнего применённого изменения (та же шкала, что
    /// lastUpdateId ответа GET /v4/public/depth — «Orderbook manage», пункты 4–6). None — снимка ещё нет. Ошибок нет.
    pub fn version(&self) -> Option<i64> {
        self.last
    }

    /// Что делает: кадр изменений. Принимает: изменения. Возвращает: Applied. Ошибок нет: разрыв номеров — Gap
    /// (стакан сброшен, отложенные кадры — тоже).
    pub fn apply(&mut self, u: Update) -> Applied {
        let Some(last) = self.last else {
            self.buffer.push(u);
            return Applied::Waiting;
        };
        if u.last <= last {
            return Applied::Repeat;
        }
        let ok = if self.first_pending {
            u.first <= last + 1
        } else {
            u.first == last + 1
        };
        if !ok {
            return self.reset(format!(
                "fi {} не продолжает номер стакана {last} (i {})",
                u.first, u.last
            ));
        }
        put(&mut self.bids, &u.bids);
        put(&mut self.asks, &u.asks);
        self.last = Some(u.last);
        self.first_pending = false;
        Applied::Changed
    }

    /// Что делает: снимок REST: уровни целиком, затем отложенные изменения по правилам. Принимает: lastUpdateId,
    /// уровни. Возвращает: Changed или Gap (отложенные не продолжают снимок). Ошибок нет.
    pub fn snapshot(&mut self, id: i64, bids: &[PriceLevel], asks: &[PriceLevel]) -> Applied {
        self.bids.clear();
        self.asks.clear();
        put(&mut self.bids, bids);
        put(&mut self.asks, asks);
        self.last = Some(id);
        self.first_pending = true;
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
        self.last = None;
        self.first_pending = false;
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
