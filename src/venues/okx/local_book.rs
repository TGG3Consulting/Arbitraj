//! Файл: venues/okx/local_book.rs
//! Что делает: локальный стакан одной пары OKX канала books (снимок 400 уровней + изменения) с проверкой номеров:
//! prevSeqId изменения должен совпасть с seqId предыдущего кадра, иначе — разрыв (пересинхронизация в ws_conn.rs).
//! Отдаёт уровни в формате ядра (bids — по убыванию цены, asks — по возрастанию).
//! Кто вызывает: ws_conn.rs (по кадру books), тесты.
//! От чего зависит: core/types (PriceLevel), depth.rs (Depth).
//! Настройки: нет.
//!
//! Правила — официальная документация https://www.okx.com/docs-v5/en/ «Order Book Trading → Market Data → WS / Order book
//! channel»: «The prevSeqId in the new message matches with seqId of the previous message»; у снимка prevSeqId = -1;
//! «The sequence number may be reset due to maintenance … users will receive an incremental message with seqId smaller
//! than prevSeqId. However, subsequent messages will follow the regular sequencing rule» — поэтому проверяется только
//! prevSeqId = прежний seqId, сам seqId может стать меньше; «If there are no updates to the depth for an extended
//! period(Around 60 seconds) … OKX will send a message with 'asks': [], 'bids': [] … seqId is the same as the last sent
//! message and prevSeqId equals to seqId» — пустой кадр без изменений. Количество 0 — уровень убирается (так сказано в
//! разделе SBE: «If the quantity is zero, remove the price level from the order book»; для JSON — то же по смыслу
//! изменения, проверить на живой записи).

use std::collections::BTreeMap;

use rust_decimal::Decimal;

use crate::core::types::PriceLevel;

use super::depth::Depth;

/// Что сделал кадр.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// Стакан изменился — отдать ядру.
    Changed,
    /// Пустой кадр «соединение живо» (seqId = prevSeqId = прежний) — только время кадра пары.
    Repeat,
    /// Разрыв: стакан недостоверен до нового снимка (причина).
    Gap(String),
}

/// Стакан пары.
#[derive(Debug, Default, Clone)]
pub struct LocalBook {
    bids: BTreeMap<Decimal, Decimal>,
    asks: BTreeMap<Decimal, Decimal>,
    /// seqId последнего применённого кадра; None — снимка ещё не было (или стакан сброшен).
    seq: Option<i64>,
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
    /// Что делает: применяет кадр books. Принимает: разобранный кадр (depth.rs). Возвращает: Applied. Ошибок нет:
    /// нет номера, изменение до снимка, prevSeqId не тот — Gap с причиной (стакан сброшен).
    pub fn apply(&mut self, d: &Depth) -> Applied {
        let Some(seq) = d.seq else {
            return self.reset("кадр без seqId".into());
        };
        if d.snapshot {
            self.bids.clear();
            self.asks.clear();
        } else {
            let Some(cur) = self.seq else {
                return self.reset("изменение (update) пришло до снимка".into());
            };
            if d.prev_seq != Some(cur) {
                return self.reset(format!(
                    "prevSeqId {} не равен прежнему seqId {cur}",
                    d.prev_seq
                        .map_or_else(|| "нет".to_string(), |p| p.to_string())
                ));
            }
            if seq == cur && d.bids.is_empty() && d.asks.is_empty() {
                return Applied::Repeat;
            }
        }
        put(&mut self.bids, &d.bids);
        put(&mut self.asks, &d.asks);
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

    /// Верхние уровни: bids по убыванию цены, asks по возрастанию, не больше depth на сторону. Ошибок нет.
    pub fn top(&self, depth: usize) -> (Vec<PriceLevel>, Vec<PriceLevel>) {
        let lv = |(p, q): (&Decimal, &Decimal)| PriceLevel { price: *p, qty: *q };
        (
            self.bids.iter().rev().take(depth).map(lv).collect(),
            self.asks.iter().take(depth).map(lv).collect(),
        )
    }

    /// seqId последнего применённого кадра. Ошибок нет.
    pub fn version(&self) -> Option<i64> {
        self.seq
    }
}
