//! Файл: venues/gate/fast_book.rs
//! Что делает: решение 421 — локальный стакан одной пары Gate по потоку spot.obu (Order Book V2): полный снимок заменяет
//! стакан, изменение применяется, только если U = номер стакана + 1; иначе — разрыв (стакан сброшен до нового полного
//! снимка, пересинхронизацию делает fast_pick.rs / ws_conn.rs). Отдаёт верхние уровни в формате ядра.
//! Решение 421 (ревью): стакан после кадра пересечён (лучшая покупка ≥ лучшей продажи) — тоже разрыв: такой стакан
//! неверен при любой целой цепочке номеров.
//! Кто вызывает: fast_pick.rs, тесты.
//! От чего зависит: obu.rs (Frame), core/types (PriceLevel).
//! Настройки: нет.
//!
//! Официально (Spot WebSocket v4, «Order Book V2 API», https://www.gate.com/docs/developers/apiv4/ws/en/), порядок
//! ведения локального стакана: «The first message after subscribing is a full snapshot» — заменить стакан, номер = u;
//! «the server may send full depth snapshots multiple times» — каждый полный снимок заменяет стакан; изменение: «if U
//! equals the local depth ID plus 1» — номер = u, изменения по цене, количество «0» — убрать уровень; иначе «you must
//! unsubscribe from the market and resubscribe to retrieve the initial depth snapshot».
//! Уровни, выпавшие из окна потока (50 или 400): удаляет ли их сервер явно — в документации не сказано (вопрос в
//! docs/TASKS.md); поэтому ядру отдаются не больше уровней потока (top), глубже — не показываются.

use std::collections::BTreeMap;

use rust_decimal::Decimal;

use crate::core::types::PriceLevel;

use super::obu::Frame;

/// Что сделал кадр.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// Стакан изменился (снимок или изменения) — можно отдавать ядру.
    Changed,
    /// Изменение до первого полного снимка — пропущено (ждём снимок после подписки).
    Waiting,
    /// Разрыв номеров: стакан сброшен до нового полного снимка (причина).
    Gap(String),
}

/// Стакан пары.
#[derive(Debug, Default, Clone)]
pub struct FastBook {
    bids: BTreeMap<Decimal, Decimal>,
    asks: BTreeMap<Decimal, Decimal>,
    /// Номер стакана (u последнего применённого кадра); None — полного снимка ещё нет.
    id: Option<i64>,
    /// t последнего применённого кадра, мс (Gate).
    t: Option<i64>,
}

/// Изменения стороны: новое количество, 0 — убрать уровень. Ошибок нет.
fn put(side: &mut BTreeMap<Decimal, Decimal>, levels: &[PriceLevel]) {
    for l in levels {
        if l.qty.is_zero() {
            side.remove(&l.price);
        } else {
            side.insert(l.price, l.qty);
        }
    }
}

impl FastBook {
    /// Синхронизирован ли стакан (полный снимок есть, разрыва после него не было). Ошибок нет.
    pub fn synced(&self) -> bool {
        self.id.is_some()
    }

    /// Номер стакана (u последнего применённого кадра). Ошибок нет.
    pub fn version(&self) -> Option<i64> {
        self.id
    }

    /// Время биржи (t) последнего применённого кадра, мс. Ошибок нет.
    pub fn time(&self) -> Option<i64> {
        self.t
    }

    /// Что делает: применяет кадр spot.obu. Принимает: кадр. Возвращает: Applied. Ошибок нет: изменение не продолжает
    /// номер стакана (U ≠ номер + 1) или без U — Gap (стакан сброшен).
    pub fn apply(&mut self, f: &Frame) -> Applied {
        if f.full {
            self.bids.clear();
            self.asks.clear();
            put(&mut self.bids, &f.bids);
            put(&mut self.asks, &f.asks);
            self.id = Some(f.last);
            self.t = f.t;
            return self.checked(f.last);
        }
        let Some(id) = self.id else {
            return Applied::Waiting;
        };
        match f.first {
            Some(u0) if u0 == id + 1 => {}
            Some(u0) => {
                return self.reset(format!(
                    "U {u0} не равен номеру стакана {id} + 1 (u {})",
                    f.last
                ));
            }
            None => return self.reset(format!("изменение без U (u {})", f.last)),
        }
        put(&mut self.bids, &f.bids);
        put(&mut self.asks, &f.asks);
        self.id = Some(f.last);
        self.t = f.t;
        self.checked(f.last)
    }

    /// Решение 421 (ревью): после кадра — пересечён ли стакан (лучшая покупка ≥ лучшей продажи). Принимает: u кадра.
    /// Возвращает: Changed или Gap (стакан сброшен). Ошибок нет.
    fn checked(&mut self, u: i64) -> Applied {
        let best = (
            self.bids.keys().next_back().copied(),
            self.asks.keys().next().copied(),
        );
        match best {
            (Some(b), Some(a)) if b >= a => self.reset(format!(
                "стакан пересечён после u {u}: покупка {b} ≥ продажа {a}"
            )),
            _ => Applied::Changed,
        }
    }

    /// Сбрасывает стакан (недостоверен до нового полного снимка). Принимает: причину. Возвращает: Gap. Ошибок нет.
    pub fn reset(&mut self, why: String) -> Applied {
        self.bids.clear();
        self.asks.clear();
        self.id = None;
        self.t = None;
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
