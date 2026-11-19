//! Файл: venues/poloniex/local_book.rs
//! Что делает: решение 424 — локальный стакан одной пары Poloniex по каналу book_lv2 (снимок 20 уровней + изменения) с
//! проверкой цепочки id/lastId. Отдаёт уровни в формате ядра (bids — по убыванию цены, asks — по возрастанию).
//! Кто вызывает: fast_book.rs (по записи book_lv2), тесты.
//! От чего зависит: core/types (PriceLevel), lv2.rs (Lv2).
//! Настройки: нет.
//!
//! Правила — официальная документация (Market Data → Book Level 2, https://api-docs.poloniex.com/spot/websocket/
//! market-data), «How to maintain a local order book»: снимок — «reset your order book data structure to match this
//! new order book»; изменение — «When quantity is positive, update the corresponding price … When quantity is 0, delete
//! this price»; «If id of the last message does not match lastId of the current message» — «the client has lost
//! connection with the server and must re-subscribe to the channel» (разрыв → Gap, переподписка — fast_book.rs).
//! Стакан канала — «full 20 level order book» (LV2_LEVELS): уровни глубже 20-го биржа не ведёт, они срезаются.
//! Проверка «лучшая покупка ≥ лучшей продажи» — не правило документации, а свойство любого верного стакана:
//! пересечённый стакан — разрыв (сверка, общая постановка 420–424 п.4).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::PriceLevel;

use super::lv2::Lv2;

/// Уровней на сторону у book_lv2 — «Receive a snapshot of the full 20 level order book» (офдока, не настройка).
pub const LV2_LEVELS: usize = 20;

/// Что сделала запись.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// Стакан синхронизирован и, возможно, изменился — можно отдать ядру.
    Changed,
    /// Стакан не синхронизирован (ждём снимок после переподписки) — изменение пропущено, это не новый разрыв.
    Waiting,
    /// Разрыв: стакан недостоверен до нового снимка (причина).
    Gap(String),
}

/// Стакан пары.
#[derive(Debug, Default, Clone)]
pub struct LocalBook {
    bids: BTreeMap<Decimal, Decimal>,
    asks: BTreeMap<Decimal, Decimal>,
    /// id последней применённой записи; None — снимка не было (или стакан сброшен).
    id: Option<i64>,
    /// createTime последней применённой записи.
    time: Option<DateTime<Utc>>,
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
    /// Что делает: применяет запись book_lv2. Принимает: разобранную запись (lv2.rs). Возвращает: Applied. Ошибок нет:
    /// нет id, lastId не равен прежнему id, пересечённый стакан — Gap с причиной (стакан сброшен); изменение без
    /// снимка — Waiting.
    pub fn apply(&mut self, d: &Lv2) -> Applied {
        let Some(id) = d.id else {
            return self.reset("запись без id".into());
        };
        if d.snapshot {
            self.bids.clear();
            self.asks.clear();
        } else {
            let Some(cur) = self.id else {
                return Applied::Waiting;
            };
            if d.last_id != Some(cur) {
                let got = d
                    .last_id
                    .map_or_else(|| "нет".to_string(), |p| p.to_string());
                return self.reset(format!("lastId {got} не равен id прежней записи {cur}"));
            }
        }
        put(&mut self.bids, &d.bids);
        put(&mut self.asks, &d.asks);
        while self.bids.len() > LV2_LEVELS {
            self.bids.pop_first();
        }
        while self.asks.len() > LV2_LEVELS {
            self.asks.pop_last();
        }
        if let (Some((b, _)), Some((a, _))) =
            (self.bids.last_key_value(), self.asks.first_key_value())
            && b >= a
        {
            let why = format!("стакан пересёкся: лучшая покупка {b} ≥ лучшей продажи {a}");
            return self.reset(why);
        }
        self.id = Some(id);
        self.time = d.exchange_time;
        Applied::Changed
    }

    /// Сбрасывает стакан (недостоверен до снимка). Возвращает: Gap с причиной. Ошибок нет.
    pub fn reset(&mut self, why: String) -> Applied {
        self.bids.clear();
        self.asks.clear();
        self.id = None;
        self.time = None;
        Applied::Gap(why)
    }

    /// Синхронизирован ли стакан (снимок был, разрыва после него нет). Ошибок нет.
    pub fn synced(&self) -> bool {
        self.id.is_some()
    }

    /// createTime последней применённой записи. Ошибок нет.
    pub fn time(&self) -> Option<DateTime<Utc>> {
        self.time
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
