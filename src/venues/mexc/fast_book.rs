//! Файл: venues/mexc/fast_book.rs
//! Что делает: решение 420 — локальный стакан одной пары MEXC из канала изменений spot@public.aggre.depth.v3.api.pb
//! строго по официальной процедуре «How to Properly Maintain a Local Copy of the Order Book» (шаги 2–7 ниже): с ухода
//! запроса снимка REST кадры копятся, пока нет снимка; снимок старше первого отложенного кадра — снимок заново; кадры с toVersion ≤
//! lastUpdateId отбрасываются; первый оставшийся с fromVersion > lastUpdateId + 1 — разрыв; дальше каждый fromVersion
//! строго равен прежнему toVersion + 1, иначе разрыв (пересинхронизация — fast_pair.rs). Количество — новое значение
//! уровня, 0 — уровень убрать. Отдаёт верхние уровни в формате ядра.
//! Кто вызывает: fast_pair.rs (кадр изменений, снимок REST), тесты.
//! От чего зависит: core/types (PriceLevel).
//! Настройки: нет.
//!
//! Официально (https://www.mexc.com/api-docs/spot-v3/websocket-market-streams/how-to-properly-maintain-a-local-copy-of-the-order-book,
//! сверено 19.11.2026): (2) «Start caching received incremental depth updates, and record the fromVersion of the first
//! push message»; (3) снимок /api/v3/depth, «record lastUpdateId»; (4) «If lastUpdateId < the fromVersion of the first
//! push, return to Step 3 and fetch the snapshot again»; (5) «discard all pushes where toVersion <= lastUpdateId»;
//! (6) «If fromVersion > lastUpdateId + 1, the data is considered discontinuous; return to Step 3 and reinitialize»;
//! (7) «set the local version to lastUpdateId … After each update, set the local version to that push's toVersion. If
//! any push does not satisfy: fromVersion = previous toVersion + 1, you must reinitialize». Diff.Depth Stream
//! (https://www.mexc.com/api-docs/spot-v3/websocket-market-streams/diffdepth-stream): количество 0 — «that price level
//! should be removed». Шаг 4 выполняется буквально («<», а не «< fromVersion − 1»).
//! Отличие от сверки окна (tap_snap.rs, решение 125, нестрогая «from ≤ c + 1»): здесь после первого кадра — только
//! строгое равенство, как в документации; логика окна не меняется.

use std::collections::BTreeMap;

use rust_decimal::Decimal;

use crate::core::types::PriceLevel;

/// Кадр изменений в числах ядра.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    /// fromVersion.
    pub from: i64,
    /// toVersion.
    pub to: i64,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    /// sendTime обёртки («Event time»), unix мс; None — поля нет.
    pub time_ms: Option<i64>,
}

/// Что сделал кадр или снимок.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// Стакан изменился (синхронизирован).
    Changed,
    /// Кадр уже учтён снимком (шаг 5) — ничего не меняет.
    Repeat,
    /// Снимка ещё нет — кадр отложен (нужен снимок REST).
    Waiting,
    /// Снимок старше первого отложенного кадра (шаг 4) — нужен снимок заново; отложенные кадры сохранены.
    Older,
    /// Разрыв (шаг 6 или 7): стакан сброшен до нового снимка (причина).
    Gap(String),
}

/// Стакан пары.
#[derive(Debug, Default, Clone)]
pub struct LocalBook {
    bids: BTreeMap<Decimal, Decimal>,
    asks: BTreeMap<Decimal, Decimal>,
    /// Локальная версия (шаг 7); None — снимка нет.
    ver: Option<i64>,
    /// sendTime последнего применённого кадра; None — после снимка кадров ещё не было.
    time_ms: Option<i64>,
    /// После снимка ещё не применено ни одного кадра (шаги 5–6).
    first_pending: bool,
    /// Запрос снимка ушёл — кадры копятся (шаг 2); до этого кадры без снимка не хранятся (они всё равно не нужны —
    /// start_buffer), чтобы очередь снимков на старте сотен пар не копила память.
    collecting: bool,
    /// Кадры до снимка (шаг 2).
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
    /// Синхронизирован ли стакан (снимок применён, разрыва не было). Ошибок нет.
    pub fn synced(&self) -> bool {
        self.ver.is_some()
    }

    /// sendTime последнего применённого кадра (unix мс); None — не было. Ошибок нет.
    pub fn time_ms(&self) -> Option<i64> {
        self.time_ms
    }

    /// Что делает: кадр изменений (шаги 2, 5–7). Принимает: кадр. Возвращает: Applied. Ошибок нет: разрыв — Gap (стакан
    /// и отложенные кадры сброшены).
    pub fn apply(&mut self, u: Update) -> Applied {
        let Some(ver) = self.ver else {
            if self.collecting {
                self.buffer.push(u);
            }
            return Applied::Waiting;
        };
        if self.first_pending {
            if u.to <= ver {
                return Applied::Repeat;
            }
            if u.from > ver + 1 {
                return self.reset(format!(
                    "первый кадр после снимка: fromVersion {} > lastUpdateId {ver} + 1",
                    u.from
                ));
            }
        } else if u.from != ver + 1 {
            return self.reset(format!(
                "fromVersion {} ≠ прежний toVersion {ver} + 1 (toVersion {})",
                u.from, u.to
            ));
        }
        put(&mut self.bids, &u.bids);
        put(&mut self.asks, &u.asks);
        self.ver = Some(u.to);
        self.time_ms = u.time_ms.or(self.time_ms);
        self.first_pending = false;
        Applied::Changed
    }

    /// Что делает: снимок REST (шаги 3–7). Принимает: lastUpdateId, уровни. Возвращает: Older (шаг 4 — снимок не
    /// применён), Gap (отложенные кадры не продолжают снимок), иначе Changed. Ошибок нет.
    pub fn snapshot(&mut self, id: i64, bids: &[PriceLevel], asks: &[PriceLevel]) -> Applied {
        if self.buffer.first().is_some_and(|f| id < f.from) {
            return Applied::Older;
        }
        self.bids.clear();
        self.asks.clear();
        put(&mut self.bids, bids);
        put(&mut self.asks, asks);
        self.ver = Some(id);
        self.time_ms = None;
        self.first_pending = true;
        self.collecting = false;
        for u in std::mem::take(&mut self.buffer) {
            if let Applied::Gap(why) = self.apply(u) {
                return Applied::Gap(why);
            }
        }
        Applied::Changed
    }

    /// Что делает: запрос снимка REST уходит — кадры с этого момента копятся (шаг 2 перед шагом 3), прежние отложенные
    /// не нужны (снимок старше новых — шаг 4). Синхронизированный стакан не трогается. Ошибок нет.
    pub fn start_buffer(&mut self) {
        if self.ver.is_none() {
            self.buffer.clear();
            self.collecting = true;
        }
    }

    /// Сбрасывает стакан (недостоверен до нового снимка). Возвращает: Gap с причиной. Ошибок нет.
    pub fn reset(&mut self, why: String) -> Applied {
        *self = Self::default();
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
