//! Файл: core/window_book.rs
//! Что делает: стакан всей глубины на момент кадра окна (дополнение решения 125) — для страницы окна: полный стакан
//! (TapView::Diff с full — BingX action «all», снимок REST MEXC) плюс применённые по порядку изменения. Кадры уже
//! разобраны адаптером своей биржи в формат ядра; здесь только общие правила по версиям (как описывают обе биржи):
//! изменение, кончающееся не позже версии стакана, — устарело, пропуск; начинается позже версии + 1 — разрыв, стакан
//! недостоверен до следующего полного; иначе уровни применяются: количество — новое значение, 0 — уровня больше нет.
//! Изменения, пришедшие до полного стакана (MEXC: снимок берётся после подписки), ждут его и применяются после.
//! Кто вызывает: core/window_view.rs, тесты.
//! От чего зависит: core/types (PriceLevel, TapView).
//! Настройки: нет.

use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::Serialize;

use crate::core::types::{PriceLevel, TapView};

/// Изменение: уровни и версии.
type Change = (Vec<PriceLevel>, Vec<PriceLevel>, Option<i64>, Option<i64>);

/// Стакан одной биржи окна.
#[derive(Debug, Default)]
pub struct Book {
    bids: BTreeMap<Decimal, Decimal>,
    asks: BTreeMap<Decimal, Decimal>,
    ver: Option<i64>,
    ok: bool,
    /// Изменения, ждущие полного стакана.
    wait: Vec<Change>,
}

/// Стакан на момент кадра — для показа.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BookAt {
    /// Стакан достоверен (полный получен, разрывов нет).
    pub ok: bool,
    /// Что сделано с кадром: «полный стакан», «применено», «устарело — пропуск», «разрыв версий …», «ждёт полного».
    pub note: String,
    pub version: Option<i64>,
    /// Всего уровней в стакане.
    pub depth: (usize, usize),
    /// Верхние уровни: bids по убыванию, asks по возрастанию.
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
}

impl Book {
    /// Что делает: применяет кадр. Принимает: разобранный кадр (не Diff — ничего), сколько верхних уровней показать.
    /// Возвращает: стакан после кадра; None — кадр не изменения стакана. Ошибок нет.
    pub fn apply(&mut self, v: &TapView, top: usize) -> Option<BookAt> {
        let TapView::Diff {
            bids,
            asks,
            full,
            from,
            to,
        } = v
        else {
            return None;
        };
        let note = if *full {
            self.bids.clear();
            self.asks.clear();
            self.put(bids, asks);
            self.ver = *to;
            self.ok = true;
            for (b, a, f, t) in std::mem::take(&mut self.wait) {
                self.change(&b, &a, f, t);
            }
            "полный стакан".to_string()
        } else if !self.ok {
            self.wait.push((bids.clone(), asks.clone(), *from, *to));
            "ждёт полного стакана".to_string()
        } else {
            self.change(bids, asks, *from, *to)
        };
        Some(self.at(note, top))
    }

    /// Одно изменение по правилам версий (см. шапку). Возвращает: что сделано. Ошибок нет.
    fn change(
        &mut self,
        bids: &[PriceLevel],
        asks: &[PriceLevel],
        from: Option<i64>,
        to: Option<i64>,
    ) -> String {
        if let (Some(cur), Some(t)) = (self.ver, to)
            && t <= cur
        {
            return "устарело — пропуск".into();
        }
        if let (Some(cur), Some(f)) = (self.ver, from)
            && f > cur + 1
        {
            self.ok = false;
            self.wait = vec![(bids.to_vec(), asks.to_vec(), from, to)];
            return format!(
                "разрыв версий: после {cur} пришло с {f} — стакан недостоверен до полного"
            );
        }
        self.put(bids, asks);
        if to.is_some() {
            self.ver = to;
        }
        "применено".into()
    }

    /// Уровни: новое количество, 0 — убрать. Ошибок нет.
    fn put(&mut self, bids: &[PriceLevel], asks: &[PriceLevel]) {
        for (side, levels) in [(&mut self.bids, bids), (&mut self.asks, asks)] {
            for l in levels {
                if l.qty.is_zero() {
                    side.remove(&l.price);
                } else {
                    side.insert(l.price, l.qty);
                }
            }
        }
    }

    /// Стакан для показа. Ошибок нет.
    fn at(&self, note: String, top: usize) -> BookAt {
        let lv = |(p, q): (&Decimal, &Decimal)| PriceLevel { price: *p, qty: *q };
        BookAt {
            ok: self.ok,
            note,
            version: self.ver,
            depth: (self.bids.len(), self.asks.len()),
            bids: self.bids.iter().rev().take(top).map(lv).collect(),
            asks: self.asks.iter().take(top).map(lv).collect(),
        }
    }
}
