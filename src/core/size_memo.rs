//! Файл: core/size_memo.rs
//! Что делает: расчёт кривой в целых шагах количества с запоминанием (решение 66): объём n — это n × шаг, каждый
//! объём считается один раз. Здесь же держится инвариант бюджета: расчёт с вложением больше бюджета — не точка
//! кривой, а Fail::TooLarge («вложение больше бюджета»), поэтому ни одна допустимая точка бюджет не превышает.
//! Кто вызывает: core/size_bounds.rs, core/size_search.rs.
//! От чего зависит: core/size.rs.
//! Настройки: нет.

use std::collections::BTreeMap;

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use super::size::{Curve, Fail, Point};

/// Расчёты кривой по номеру шага.
pub struct Memo<'a, C: Curve> {
    curve: &'a mut C,
    pub step: Decimal,
    pub budget: Decimal,
    pub map: BTreeMap<u64, Result<Point<C::Data>, Fail>>,
    /// Предел расчётов (size.max_evals) и сколько уже посчитано.
    max: u32,
    done: u32,
}

impl<'a, C: Curve> Memo<'a, C> {
    /// Принимает: кривую, шаг (> 0), бюджет, предел расчётов. Ошибок нет.
    pub fn new(curve: &'a mut C, step: Decimal, budget: Decimal, max: u32) -> Self {
        Memo {
            curve,
            step,
            budget,
            map: BTreeMap::new(),
            max,
            done: 0,
        }
    }

    /// Предел расчётов исчерпан. Ошибок нет.
    pub fn capped(&self) -> bool {
        self.done >= self.max
    }

    /// Номер шага объёма с округлением вниз. Принимает: объём ≥ 0. Возвращает: n; None — объём не помещается в u64.
    pub fn units(&self, qty: Decimal) -> Option<u64> {
        (qty / self.step).floor().to_u64()
    }

    /// Объём n шагов. Ошибок нет.
    pub fn qty(&self, n: u64) -> Decimal {
        Decimal::from(n) * self.step
    }

    /// Расчёт на n шагов (один раз на n). Принимает: n. Возвращает: точку или Fail; n = 0 — TooSmall;
    /// списание больше бюджета — TooLarge (инвариант бюджета: проверяется в каждой точке, не только на границе —
    /// вложение от объёма у умного маршрута DEX может быть немонотонным); предел size.max_evals исчерпан — Stop.
    pub async fn at(&mut self, n: u64) -> Result<Point<C::Data>, Fail> {
        if let Some(r) = self.map.get(&n) {
            return r.clone();
        }
        if n > 0 && self.capped() {
            return Err(Fail::Stop(format!(
                "исчерпан предел расчётов size.max_evals = {}",
                self.max
            )));
        }
        let r = if n == 0 {
            Err(Fail::TooSmall("объём 0".into()))
        } else {
            let q = self.qty(n);
            self.done += 1;
            match self.curve.at(q).await {
                Ok(p) if p.invested > self.budget => Err(Fail::TooLarge(format!(
                    "объём {q}: вложение {} больше бюджета {}",
                    p.invested, self.budget
                ))),
                r => r,
            }
        };
        self.map.insert(n, r.clone());
        r
    }

    /// Прибыль на n шагов; объём недопустим или предел расчётов исчерпан — None. Ошибки: Fail::Stop (расчёт
    /// невозможен).
    pub async fn profit(&mut self, n: u64) -> Result<Option<Decimal>, Fail> {
        if !self.map.contains_key(&n) && self.capped() {
            return Ok(None);
        }
        match self.at(n).await {
            Ok(p) => Ok(Some(p.profit)),
            Err(Fail::Stop(e)) => Err(Fail::Stop(e)),
            Err(_) => Ok(None),
        }
    }

    /// Сколько объёмов посчитано (без n = 0). Ошибок нет.
    pub fn evals(&self) -> u32 {
        self.map.keys().filter(|n| **n > 0).count() as u32
    }

    /// Лучшая допустимая точка из посчитанных: наибольшая прибыль, при равной — меньший объём. None — ни одной.
    pub fn best(&self) -> Option<Point<C::Data>> {
        let mut best: Option<&Point<C::Data>> = None;
        for p in self.map.values().flatten() {
            if best.is_none_or(|b| p.profit > b.profit) {
                best = Some(p);
            }
        }
        best.cloned()
    }
}
