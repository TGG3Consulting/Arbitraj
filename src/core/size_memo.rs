//! Файл: core/size_memo.rs
//! Что делает: расчёт кривой в целых шагах количества с запоминанием (решение 66): объём n — это n × шаг, каждый
//! объём считается один раз. Здесь же держится инвариант бюджета: расчёт с вложением больше бюджета — не точка
//! кривой, а Fail::OverBudget («вложение больше бюджета»), поэтому ни одна допустимая точка бюджет не превышает.
//! Решение 246: текст отказа «вложение больше бюджета» — over_budget, общий с ранней проверкой costs.rs evaluate_capped.
//! Решение 265: каждый расчёт объёма (at, beyond) — часть подробного замера «расчёт объёма» (calc_detail.rs, только при log.step_timing).
//! Решение 336: короб подбора для адаптеров (gateway/calc_share.rs) — только когда нужен (точки параллельно или
//! dex.quote_reuse; ставит size_par.rs with_share), иначе расчёт точки как до 336; пачки точек — size_par.rs at_many.
//! Решение 345 (задача t159, ревью t159 Б2): мягкий срок подбора — size_gate.rs (ставится после границ и действует,
//! только когда посчитан объём, проходящий оба порога); после него новых точек нет, как при исчерпанном size.max_evals
//! (capped); точка «вне предела» (force, решение 237) срок не останавливает. Порог % — size_gate.rs best_pct.
//! Кто вызывает: core/size_bounds.rs, core/size_search.rs, core/size_par.rs; over_budget — ещё core/costs.rs.
//! От чего зависит: core/size.rs, core/calc_timing.rs, gateway/calc_share.rs.
//! Настройки: нет (size.parallel_points и признак dex.quote_reuse — через Params, size_par.rs with_share).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::gateway::calc_share::{self, Share};

use super::calc_detail::stage_detail;
use super::size::{Curve, Fail, Point};

/// Решение 265: имя части замера «один расчёт объёма» (calc_detail.rs) — его же ищет calc_steps.rs.
pub const EVAL: &str = "расчёт объёма";
/// Решение 336: имя части замера «пачка расчётов объёма» (size.parallel_points > 1) — время пачки по часам стены.
pub const EVAL_BATCH: &str = "пачка расчётов объёма";

/// Что делает: отказ «вложение больше бюджета» — один вид и текст у проверки точки (Memo::at) и у ранней проверки
/// расчёта (решение 246, costs.rs evaluate_capped). Принимает: объём, списание, бюджет. Возвращает: Fail::OverBudget
/// (ревью t159 Н-4: свой вид, size_scan.rs отличает его от иных «слишком большой» без разбора текста). Ошибок нет.
pub fn over_budget(qty: Decimal, invested: Decimal, budget: Decimal) -> Fail {
    Fail::OverBudget(format!(
        "объём {qty}: вложение {invested} больше бюджета {budget}"
    ))
}

/// Расчёты кривой по номеру шага.
pub struct Memo<'a, C: Curve> {
    pub(super) curve: &'a mut C,
    pub step: Decimal,
    pub budget: Decimal,
    pub map: BTreeMap<u64, Result<Point<C::Data>, Fail>>,
    /// Предел расчётов (size.max_evals) и сколько уже посчитано.
    pub(super) max: u32,
    pub(super) done: u32,
    /// Решение 336: сколько точек считать за раз (size.parallel_points; 1 — по одной).
    pub(super) par: usize,
    /// Решение 336: короб этого подбора для адаптеров (gateway/calc_share.rs); None — не нужен (ревью t157 Б1: точки по
    /// одной и повторное использование не включено — расчёт точки как до 336).
    pub(super) share: Option<Arc<Share>>,
    /// Решение 345: мягкий срок подбора (ставит size_gate.rs arm после границ); None — без срока (как до t159).
    pub(super) deadline: Option<Instant>,
    /// Решение 342 / 345: пороги направления — признак «точка проходит оба порога» (size_gate.rs).
    pub(super) gate: super::size_gate::Gate,
    /// Решение 345: среди посчитанных есть точка, проходящая оба порога (ведётся только при заданном сроке).
    pub(super) passed: bool,
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
            par: 1,
            share: None,
            deadline: None,
            gate: Default::default(),
            passed: false,
        }
    }

    /// Предел расчётов исчерпан. Ошибок нет.
    pub fn capped(&self) -> bool {
        self.done >= self.max || self.late()
    }

    /// Номер шага объёма с округлением вниз. Принимает: объём ≥ 0. Возвращает: n; None — объём не помещается в u64.
    pub fn units(&self, qty: Decimal) -> Option<u64> {
        (qty / self.step).floor().to_u64()
    }

    /// Объём n шагов. Ошибок нет.
    pub fn qty(&self, n: u64) -> Decimal {
        Decimal::from(n) * self.step
    }

    /// Решение 237: расчёт на n шагов ВНЕ предела расчётов (объём, на котором вошли в сделку, — считается всегда, даже
    /// если предел уже исчерпан; в счёт предела идёт). Принимает: n. Возвращает: как at. Ошибок сверх at нет.
    pub async fn force(&mut self, n: u64) -> Result<Point<C::Data>, Fail> {
        if self.map.contains_key(&n) || !self.capped() {
            return self.at(n).await;
        }
        let (max, deadline) = (self.max, self.deadline.take());
        self.max = self.done + 1;
        let r = self.at(n).await;
        self.max = max;
        self.deadline = deadline;
        r
    }

    /// Расчёт на n шагов (один раз на n). Принимает: n. Возвращает: точку или Fail; n = 0 — TooSmall;
    /// списание больше бюджета — OverBudget (инвариант бюджета: проверяется в каждой точке, не только на границе —
    /// вложение от объёма у умного маршрута DEX может быть немонотонным); предел size.max_evals исчерпан — Stop.
    pub async fn at(&mut self, n: u64) -> Result<Point<C::Data>, Fail> {
        if let Some(r) = self.map.get(&n) {
            return r.clone();
        }
        if n > 0 && self.capped() {
            // Решение 345: мягкий срок — свой текст. Сюда срок не доходит: он ставится после границ (size_gate.rs arm),
            // а дальше точки берутся через profit / at_many / value, которые при capped новых точек не просят.
            if self.late() {
                return Err(Fail::Stop(
                    "наступил мягкий срок подбора size.soft_deadline_pct".into(),
                ));
            }
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
            // Уступаем рабочий поток tokio перед тяжёлым вычислением точки (решение 90): параллельные расчёты
            // (решение 88) после ухода ожидания узла (решение 89) идут сплошным CPU и без этого монополизируют
            // потоки рантайма — голодали бы веб-сервер, обработка событий пулов и опрос блока.
            tokio::task::yield_now().await;
            // Решение 265: каждый расчёт объёма — часть замера (сколько раз и сколько в среднем).
            match self.eval(q).await {
                Ok(p) if p.invested > self.budget => Err(over_budget(q, p.invested, self.budget)),
                r => r,
            }
        };
        // Решение 345: признак «проходящий оба порога посчитан» (только при сроке).
        self.note(&r);
        self.map.insert(n, r.clone());
        r
    }

    /// Расчёт на n шагов без проверки бюджета и без памяти — чем ограничен поиск сверху (решение 74, size_max.rs).
    /// Принимает: n. Возвращает: точку или Fail кривой. Ошибок сверх кривой нет.
    pub async fn beyond(&mut self, n: u64) -> Result<Point<C::Data>, Fail> {
        let q = self.qty(n);
        // Уступаем поток перед тяжёлым вычислением точки (решение 90) — см. at().
        tokio::task::yield_now().await;
        self.eval(q).await
    }

    /// Один расчёт кривой под замером «расчёт объёма» (решение 265) и, если короб есть, под ним (решение 336; нет —
    /// как до 336). Принимает: объём. Возвращает: итог кривой. Ошибок сверх кривой нет.
    async fn eval(&mut self, q: Decimal) -> Result<Point<C::Data>, Fail> {
        match self.share.clone() {
            Some(s) => stage_detail(|| EVAL.into(), calc_share::scope(s, self.curve.at(q))).await,
            None => stage_detail(|| EVAL.into(), self.curve.at(q)).await,
        }
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
