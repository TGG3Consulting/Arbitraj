//! Файл: venues/kraken/local_book.rs
//! Что делает: локальный стакан одной пары Kraken канала book (снимок + изменения) с проверкой чек-суммы CRC32 верхних
//! 10 уровней после каждого кадра: не совпала — разрыв (пересинхронизация в ws_conn.rs). Стакан держится не глубже
//! подписанной глубины. Отдаёт уровни в формате ядра (bids — по убыванию цены, asks — по возрастанию).
//! Кто вызывает: ws_conn.rs (по кадру book), тесты.
//! От чего зависит: core/types (PriceLevel), depth.rs (Depth), checksum.rs.
//! Настройки: нет.
//!
//! Правила — https://docs.kraken.com/exchange/guides/websockets/book-checksum-v2.md: «Process all updates in a message
//! before calculating, and remove any level with qty 0»; чек-сумма — по верхним 10 уровням каждой стороны независимо от
//! глубины подписки. Обрезка до подписанной глубины после изменения в прочитанных страницах не описана — делается: на
//! чек-сумму (10 уровней) не влияет, а уровни глубже подписки сервер больше не обновляет (их не считаем, чтобы котировка
//! не опиралась на устаревшее — проверить на живой записи).

use std::collections::BTreeMap;

use rust_decimal::Decimal;

use crate::core::types::PriceLevel;

use super::checksum;
use super::depth::Depth;

/// Что сделал кадр.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// Стакан изменился и чек-сумма совпала — отдать ядру.
    Changed,
    /// Разрыв: стакан недостоверен до нового снимка (причина).
    Gap(String),
}

/// Стакан пары.
#[derive(Debug, Clone)]
pub struct LocalBook {
    bids: BTreeMap<Decimal, Decimal>,
    asks: BTreeMap<Decimal, Decimal>,
    /// Снимок был (иначе изменения не применяются).
    ready: bool,
    /// Подписанная глубина (уровней на сторону).
    depth: usize,
    price_dp: u32,
    lot_dp: u32,
}

impl LocalBook {
    /// Пустой стакан. Принимает: подписанную глубину, знаки цены и количества пары (формат чек-суммы). Ошибок нет.
    pub fn new(depth: usize, price_dp: u32, lot_dp: u32) -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            ready: false,
            depth,
            price_dp,
            lot_dp,
        }
    }

    /// Что делает: применяет кадр book и сверяет чек-сумму. Принимает: разобранный кадр. Возвращает: Applied.
    /// Ошибок нет: изменение до снимка, нет checksum, чек-сумма не та — Gap с причиной (стакан сброшен).
    pub fn apply(&mut self, d: &Depth) -> Applied {
        if d.snapshot {
            self.bids.clear();
            self.asks.clear();
            self.ready = true;
        } else if !self.ready {
            return self.reset("изменение (update) пришло до снимка".into());
        }
        put(&mut self.bids, &d.bids);
        put(&mut self.asks, &d.asks);
        self.trim();
        let Some(want) = d.checksum else {
            return self.reset("кадр без checksum".into());
        };
        let (bids, asks) = self.top(checksum::LEVELS);
        let got = checksum::of(&asks, &bids, self.price_dp, self.lot_dp);
        if got != want {
            return self.reset(format!("чек-сумма {got} не равна checksum Kraken {want}"));
        }
        Applied::Changed
    }

    /// Обрезает стороны до подписанной глубины (худшие уровни). Ошибок нет.
    fn trim(&mut self) {
        while self.bids.len() > self.depth {
            self.bids.pop_first();
        }
        while self.asks.len() > self.depth {
            self.asks.pop_last();
        }
    }

    /// Сбрасывает стакан (недостоверен до снимка). Возвращает: Gap с причиной. Ошибок нет.
    fn reset(&mut self, why: String) -> Applied {
        self.bids.clear();
        self.asks.clear();
        self.ready = false;
        Applied::Gap(why)
    }

    /// Верхние уровни: bids по убыванию цены, asks по возрастанию, не больше n на сторону. Ошибок нет.
    pub fn top(&self, n: usize) -> (Vec<PriceLevel>, Vec<PriceLevel>) {
        let lv = |(p, q): (&Decimal, &Decimal)| PriceLevel { price: *p, qty: *q };
        (
            self.bids.iter().rev().take(n).map(lv).collect(),
            self.asks.iter().take(n).map(lv).collect(),
        )
    }
}

/// Уровни стороны по порядку кадра: новое количество, 0 — убрать. Ошибок нет.
fn put(side: &mut BTreeMap<Decimal, Decimal>, levels: &[PriceLevel]) {
    for l in levels {
        if l.qty.is_zero() {
            side.remove(&l.price);
        } else {
            side.insert(l.price, l.qty);
        }
    }
}
