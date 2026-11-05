//! Файл: venues/phemex/local_book.rs
//! Что делает: локальный стакан одной пары Phemex (снимок + изменения) в масштабированных целых с проверкой номера
//! sequence; отдаёт верхние уровни в формате ядра (bids — по убыванию цены, asks — по возрастанию), переводя в Decimal
//! только их (урок 13: без копий всего стакана на кадр).
//! Кто вызывает: ws_conn.rs (по кадру стакана), тесты.
//! От чего зависит: depth.rs (Frame), num.rs (unscale), core/types (PriceLevel).
//! Настройки: нет (строгость проверки номера передаёт ws_conn.rs из venue.book_seq_strict).
//!
//! Официально (Spot Websocket API → OrderBook message, https://phemex-docs.github.io/#orderbook-message): снимок сразу
//! после подписки, затем incremental; «qty 0 indicates price level deletion»; «Snapshot messages are published with
//! 60-second interval for client self-verification»; sequence — «Latest message sequence». Идут ли номера пары подряд
//! (+1), документация не говорит: по умолчанию проверяется только рост номера, строго +1 — при venue.book_seq_strict
//! = да (как Bybit, решение 136; проверить на записанных кадрах). Разрыв: стакан пары недостоверен до следующего
//! снимка — пересподписка одной пары у Phemex невозможна («orderbook.unsubscribe … unsubscribes all orderbook related
//! subscriptions»), а снимок приходит сам не реже раза в 60 с (без бесконечного круга ресинков, урок 12).

use std::collections::BTreeMap;

use crate::core::types::PriceLevel;

use super::depth::{Frame, RawLevel};
use super::num;

/// Что сделал кадр.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// Стакан изменился — отдать ядру.
    Changed,
    /// Изменение до снимка (после разрыва) — пропущено, стакан по-прежнему недостоверен.
    Waiting,
    /// Разрыв: стакан недостоверен до нового снимка (причина).
    Gap(String),
}

/// Стакан пары.
#[derive(Debug, Default, Clone)]
pub struct LocalBook {
    bids: BTreeMap<i64, i64>,
    asks: BTreeMap<i64, i64>,
    /// sequence последнего применённого кадра; None — снимка ещё не было (или стакан сброшен).
    seq: Option<i64>,
}

/// Уровни стороны: новое количество, 0 — убрать. Ошибок нет.
fn put(side: &mut BTreeMap<i64, i64>, levels: &[RawLevel]) {
    for &(p, q) in levels {
        if q == 0 {
            side.remove(&p);
        } else {
            side.insert(p, q);
        }
    }
}

impl LocalBook {
    /// Что делает: применяет кадр. Принимает: кадр, строгость номера (+1). Возвращает: Applied. Ошибок нет: нет номера,
    /// номер не вырос (или не +1 при строгой проверке) — Gap с причиной (стакан сброшен).
    pub fn apply(&mut self, f: &Frame, strict: bool) -> Applied {
        let Some(seq) = f.seq else {
            return self.reset("кадр без sequence".into());
        };
        if f.snapshot {
            self.bids.clear();
            self.asks.clear();
        } else {
            let Some(cur) = self.seq else {
                return Applied::Waiting;
            };
            let ok = if strict { seq == cur + 1 } else { seq > cur };
            if !ok {
                let rule = if strict {
                    "прежний + 1"
                } else {
                    "больше прежнего"
                };
                return self.reset(format!("sequence {seq} не {rule} ({cur})"));
            }
        }
        put(&mut self.bids, &f.bids);
        put(&mut self.asks, &f.asks);
        self.seq = Some(seq);
        Applied::Changed
    }

    /// Сбрасывает стакан (недостоверен до снимка). Возвращает: Gap с причиной. Ошибок нет.
    fn reset(&mut self, why: String) -> Applied {
        self.bids.clear();
        self.asks.clear();
        self.seq = None;
        Applied::Gap(why)
    }

    /// Верхние уровни настоящими числами: bids по убыванию цены, asks по возрастанию, не больше depth на сторону.
    /// Принимает: глубину, масштаб цены, масштаб количества. Ошибок нет (уровень вне масштаба Decimal пропускается).
    pub fn top(
        &self,
        depth: usize,
        price_scale: u32,
        qty_scale: u32,
    ) -> (Vec<PriceLevel>, Vec<PriceLevel>) {
        let lv = |(p, q): (&i64, &i64)| {
            Some(PriceLevel {
                price: num::unscale(i128::from(*p), price_scale)?,
                qty: num::unscale(i128::from(*q), qty_scale)?,
            })
        };
        (
            self.bids.iter().rev().take(depth).filter_map(lv).collect(),
            self.asks.iter().take(depth).filter_map(lv).collect(),
        )
    }

    /// sequence последнего применённого кадра. Ошибок нет.
    pub fn version(&self) -> Option<i64> {
        self.seq
    }
}
