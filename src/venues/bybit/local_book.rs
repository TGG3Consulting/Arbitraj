//! Файл: venues/bybit/local_book.rs
//! Что делает: локальный стакан одной пары Bybit из снимка и изменений (snapshot + delta) с проверкой номера u:
//! изменение до снимка или с номером не больше прежнего — разрыв (пересинхронизация в ws_conn.rs). Отдаёт верхние N
//! уровней в формате ядра (bids — по убыванию цены, asks — по возрастанию).
//! Кто вызывает: ws_conn.rs (по кадру стакана), тесты.
//! От чего зависит: core/types (PriceLevel), num.rs.
//! Настройки: нет своих (глубину и venue.book_seq_strict передаёт ws_conn.rs).
//!
//! Правила — официальная документация Orderbook https://bybit-exchange.github.io/docs/v5/websocket/public/orderbook,
//! «Process snapshot/delta»: «Once you have subscribed successfully, you will receive a snapshot… If you receive a new
//! snapshot message, you will have to reset your local orderbook»; «If you receive an amount that is 0, delete the
//! entry; If you receive an amount that does not exist, insert it; If the entry exists, you simply update the value»;
//! «spot level 1 data: if 3 seconds have elapsed without a change… a snapshot message will be pushed again, and the
//! field u will be the same as that in the previous message». Что номер u у спота растёт строго на 1 — в документации
//! НЕ сказано; на живой записи 04.11 шёл строго +1. Строгая проверка — настройка venue.book_seq_strict [bybit]
//! (решение 136, Тигран 04.11): да — изменение с u ≠ прежний + 1 — разрыв; нет или не задана — только рост u.
//! Разрыв → пересинхронизация в ws_conn.rs (повторный разрыв раньше venue.ws_reconnect_pause_ms — переподключение,
//! без бесконечного круга пересподписок).

use std::collections::BTreeMap;
use std::str::FromStr;

use rust_decimal::Decimal;

use crate::core::types::PriceLevel;

/// Что сделал кадр.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// Стакан изменился — отдать ядру.
    Changed,
    /// Повтор снимка с тем же u (уровень 1 раз в 3 с) — только время кадра пары.
    Repeat,
    /// Разрыв: стакан недостоверен до нового снимка (причина).
    Gap(String),
}

/// Уровни стороны: новое количество, 0 — убрать. Ошибка — текст (уровень не число).
fn put(side: &mut BTreeMap<Decimal, Decimal>, levels: &[[&str; 2]]) -> Result<(), String> {
    for [p, q] in levels {
        let (Ok(price), Ok(qty)) = (Decimal::from_str(p.trim()), Decimal::from_str(q.trim()))
        else {
            return Err(format!("уровень [{p}, {q}] не число"));
        };
        if qty.is_zero() {
            side.remove(&price);
        } else {
            side.insert(price, qty);
        }
    }
    Ok(())
}

/// Стакан пары.
#[derive(Debug, Default, Clone)]
pub struct LocalBook {
    bids: BTreeMap<Decimal, Decimal>,
    asks: BTreeMap<Decimal, Decimal>,
    /// Номер u последнего применённого кадра; None — снимка ещё не было (или стакан сброшен).
    u: Option<i64>,
}

impl LocalBook {
    /// Что делает: применяет кадр. Принимает: снимок ли, номер u, строгую ли проверку u (+1), уровни [цена,
    /// количество] строками. Возвращает: Applied. Ошибок нет: неразборчивое число — Gap с причиной (стакан сброшен).
    pub fn apply(
        &mut self,
        snapshot: bool,
        u: i64,
        strict: bool,
        bids: &[[&str; 2]],
        asks: &[[&str; 2]],
    ) -> Applied {
        if snapshot {
            if self.u == Some(u) {
                return Applied::Repeat;
            }
            self.bids.clear();
            self.asks.clear();
        } else {
            match self.u {
                None => return self.reset("изменение (delta) пришло до снимка".into()),
                Some(cur) if u <= cur => {
                    return self.reset(format!("номер u не вырос: был {cur}, пришёл {u}"));
                }
                Some(cur) if strict && u != cur + 1 => {
                    return self.reset(format!(
                        "пропуск номера u: был {cur}, пришёл {u} (venue.book_seq_strict = да)"
                    ));
                }
                _ => {}
            }
        }
        if let Err(why) = put(&mut self.bids, bids).and_then(|_| put(&mut self.asks, asks)) {
            return self.reset(why);
        }
        self.u = Some(u);
        Applied::Changed
    }

    /// Сбрасывает стакан (недостоверен до снимка). Возвращает: Gap с причиной. Ошибок нет.
    fn reset(&mut self, why: String) -> Applied {
        self.bids.clear();
        self.asks.clear();
        self.u = None;
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

    /// Номер u последнего применённого кадра. Ошибок нет.
    pub fn version(&self) -> Option<i64> {
        self.u
    }
}
