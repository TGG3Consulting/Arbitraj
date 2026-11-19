//! Файл: venues/htx/fast_book.rs
//! Что делает: решение 422 — локальный стакан одной пары HTX по процедуре «Market By Price (incremental update)»:
//! изменения копятся, пока не придёт снимок по запросу того же канала; снимок выравнивается по seqNum (изменения с
//! seqNum ≤ seqNum снимка отбрасываются); дальше prevSeqNum каждого изменения должен равняться seqNum предыдущего
//! (снимок — тоже «предыдущее сообщение»), иначе разрыв. Количество — новое значение, 0 — убрать уровень; все уровни
//! кадра — сразу. Стакан держит не больше уровней канала (5, 20 или 150). Отдаёт верхние уровни в формате ядра.
//! Кто вызывает: fast_pair.rs, тесты.
//! От чего зависит: core/types (PriceLevel).
//! Настройки: нет (число уровней передаёт вызывающий).
//!
//! Официально (https://huobiapi.github.io/docs/spot/v1/en/, «Market By Price (incremental update)», шаги 1–8):
//! кэшировать изменения; запросить снимок с тем же числом уровней и выровнять по seqNum; «The "prevSeqNum" of the
//! current incremental message must be the same with "seqNum" of the previous message, otherwise it implicates message
//! loss which should require another round of refresh message retrieval and alignment»; вставить / заменить / удалить
//! (size 0) уровень. Не описано (проверить на живой записи, docs/TASKS.md): присылает ли канал удаление уровня,
//! вытесненного за предел N, — поэтому стакан обрезается до N уровней после каждого кадра.

use std::collections::BTreeMap;

use rust_decimal::Decimal;

use crate::core::types::PriceLevel;

/// Изменения одного кадра в числах ядра.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub seq: i64,
    pub prev: i64,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    /// Время биржи кадра (мс).
    pub ts: Option<i64>,
}

/// Что сделал кадр или снимок.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// Стакан изменился — отдать ядру.
    Changed,
    /// Цепочка номеров продолжена, уровней в кадре нет (у 150 уровней — «blank bids and asks» раз в 100 мс):
    /// стакан тот же — только время кадра пары.
    Same,
    /// Кадр уже учтён (seqNum ≤ номера стакана).
    Repeat,
    /// Снимка ещё нет — кадр отложен.
    Waiting,
    /// Разрыв: стакан недостоверен до нового снимка (причина).
    Gap(String),
}

/// Стакан пары.
#[derive(Debug, Clone)]
pub struct LocalBook {
    bids: BTreeMap<Decimal, Decimal>,
    asks: BTreeMap<Decimal, Decimal>,
    /// seqNum последнего учтённого сообщения; None — снимка ещё нет.
    last: Option<i64>,
    /// Время биржи последнего применённого изменения (у ответа на запрос снимка ts нет).
    ts: Option<i64>,
    /// Уровней на сторону (уровни канала).
    cap: usize,
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
    /// Пустой стакан на cap уровней стороны. Ошибок нет.
    pub fn new(cap: usize) -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            last: None,
            ts: None,
            cap,
            buffer: Vec::new(),
        }
    }

    /// Синхронизирован ли стакан (снимок есть). Ошибок нет.
    pub fn synced(&self) -> bool {
        self.last.is_some()
    }

    /// Время биржи последнего применённого изменения (мс); None — после снимка изменений ещё не было. Ошибок нет.
    pub fn ts(&self) -> Option<i64> {
        self.ts
    }

    /// Кадр изменений. Принимает: изменения. Возвращает: Applied. Ошибок нет: разрыв — Gap (стакан сброшен).
    pub fn apply(&mut self, u: Update) -> Applied {
        let Some(last) = self.last else {
            self.buffer.push(u);
            return Applied::Waiting;
        };
        if u.seq <= last {
            return Applied::Repeat;
        }
        if u.prev != last {
            return self.reset(format!(
                "prevSeqNum {} не равен seqNum предыдущего сообщения {last} (seqNum {})",
                u.prev, u.seq
            ));
        }
        self.last = Some(u.seq);
        if u.ts.is_some() {
            self.ts = u.ts;
        }
        if u.bids.is_empty() && u.asks.is_empty() {
            return Applied::Same;
        }
        put(&mut self.bids, &u.bids);
        put(&mut self.asks, &u.asks);
        self.trim();
        Applied::Changed
    }

    /// Снимок по запросу: уровни целиком, затем отложенные изменения новее снимка по цепочке. Принимает: seqNum,
    /// уровни. Возвращает: Changed или Gap (отложенное не продолжает снимок). Ошибок нет.
    pub fn snapshot(&mut self, seq: i64, bids: &[PriceLevel], asks: &[PriceLevel]) -> Applied {
        self.bids.clear();
        self.asks.clear();
        put(&mut self.bids, bids);
        put(&mut self.asks, asks);
        self.trim();
        self.last = Some(seq);
        self.ts = None;
        for u in std::mem::take(&mut self.buffer) {
            if let Applied::Gap(why) = self.apply(u) {
                return Applied::Gap(why);
            }
        }
        Applied::Changed
    }

    /// Сбрасывает стакан (недостоверен до нового снимка; кадры после сброса снова копятся). Возвращает: Gap с
    /// причиной. Ошибок нет.
    pub fn reset(&mut self, why: String) -> Applied {
        self.bids.clear();
        self.asks.clear();
        self.last = None;
        self.ts = None;
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

    /// Обрезает стороны до cap лучших уровней (bids — худшие снизу, asks — сверху). Ошибок нет.
    fn trim(&mut self) {
        while self.bids.len() > self.cap {
            self.bids.pop_first();
        }
        while self.asks.len() > self.cap {
            self.asks.pop_last();
        }
    }
}
