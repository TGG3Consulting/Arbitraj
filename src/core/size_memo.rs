//! Файл: core/size_memo.rs
//! Что делает: расчёт кривой в целых шагах количества с запоминанием (решение 66): объём n — это n × шаг, каждый
//! объём считается один раз. Здесь же держится инвариант бюджета: расчёт с вложением больше бюджета — не точка
//! кривой, а Fail::TooLarge («вложение больше бюджета»), поэтому ни одна допустимая точка бюджет не превышает.
//! Решение 246: текст отказа «вложение больше бюджета» — over_budget, общий с ранней проверкой costs.rs evaluate_capped.
//! Решение 265: каждый расчёт объёма (at, beyond) — часть подробного замера «расчёт объёма» (calc_detail.rs, только при log.step_timing).
//! Решение 336: at_many — набор независимых объёмов пачками по size.parallel_points (Curve::batch: кривая ядра считает
//! пачку одновременно); у каждого подбора свой короб для адаптеров (gateway/calc_share.rs) — им обёрнут каждый расчёт.
//! Кто вызывает: core/size_bounds.rs, core/size_search.rs, core/size_par.rs; over_budget — ещё core/costs.rs.
//! От чего зависит: core/size.rs, core/calc_timing.rs, gateway/calc_share.rs.
//! Настройки: нет (size.parallel_points — через Params, with_par).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::gateway::calc_share::{self, Share};

use super::calc_detail::stage_detail;
use super::size::{Curve, Fail, Point};

/// Решение 265: имя части замера «один расчёт объёма» (calc_detail.rs) — его же ищет calc_steps.rs.
pub const EVAL: &str = "расчёт объёма";
/// Решение 336: имя части замера «пачка расчётов объёма» (size.parallel_points > 1) — время пачки по часам стены.
pub const EVAL_BATCH: &str = "пачка расчётов объёма";

/// Что делает: отказ «вложение больше бюджета» — один текст у проверки точки (Memo::at) и у ранней проверки расчёта
/// (решение 246, costs.rs evaluate_capped). Принимает: объём, списание, бюджет. Возвращает: Fail::TooLarge. Ошибок нет.
pub fn over_budget(qty: Decimal, invested: Decimal, budget: Decimal) -> Fail {
    Fail::TooLarge(format!(
        "объём {qty}: вложение {invested} больше бюджета {budget}"
    ))
}

/// Расчёты кривой по номеру шага.
pub struct Memo<'a, C: Curve> {
    curve: &'a mut C,
    pub step: Decimal,
    pub budget: Decimal,
    pub map: BTreeMap<u64, Result<Point<C::Data>, Fail>>,
    /// Предел расчётов (size.max_evals) и сколько уже посчитано.
    max: u32,
    done: u32,
    /// Решение 336: сколько точек считать за раз (size.parallel_points; 1 — по одной).
    par: usize,
    /// Решение 336: короб этого подбора для адаптеров (gateway/calc_share.rs).
    share: Arc<Share>,
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
            share: Arc::new(Share::new(false)),
        }
    }

    /// Решение 336: те же расчёты с пачками по `par` точек (size.parallel_points). Принимает: par (0 и 1 — по одной).
    /// Возвращает: себя. Ошибок нет.
    pub fn with_par(mut self, par: u32) -> Self {
        self.par = usize::try_from(par.max(1)).unwrap_or(1);
        self.share = Arc::new(Share::new(self.par > 1));
        self
    }

    /// Решение 336: сколько точек считается за раз. Ошибок нет.
    pub fn par(&self) -> usize {
        self.par
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

    /// Решение 237: расчёт на n шагов ВНЕ предела расчётов (объём, на котором вошли в сделку, — считается всегда, даже
    /// если предел уже исчерпан; в счёт предела идёт). Принимает: n. Возвращает: как at. Ошибок сверх at нет.
    pub async fn force(&mut self, n: u64) -> Result<Point<C::Data>, Fail> {
        if self.map.contains_key(&n) || !self.capped() {
            return self.at(n).await;
        }
        let max = self.max;
        self.max = self.done + 1;
        let r = self.at(n).await;
        self.max = max;
        r
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
            // Уступаем рабочий поток tokio перед тяжёлым вычислением точки (решение 90): параллельные расчёты
            // (решение 88) после ухода ожидания узла (решение 89) идут сплошным CPU и без этого монополизируют
            // потоки рантайма — голодали бы веб-сервер, обработка событий пулов и опрос блока.
            tokio::task::yield_now().await;
            // Решение 265: каждый расчёт объёма — часть замера (сколько раз и сколько в среднем).
            // Решение 336: расчёт — под коробом подбора (gateway/calc_share.rs).
            let work = calc_share::scope(self.share.clone(), self.curve.at(q));
            match stage_detail(|| EVAL.into(), work).await {
                Ok(p) if p.invested > self.budget => Err(over_budget(q, p.invested, self.budget)),
                r => r,
            }
        };
        self.map.insert(n, r.clone());
        r
    }

    /// Расчёт на n шагов без проверки бюджета и без памяти — чем ограничен поиск сверху (решение 74, size_max.rs).
    /// Принимает: n. Возвращает: точку или Fail кривой. Ошибок сверх кривой нет.
    pub async fn beyond(&mut self, n: u64) -> Result<Point<C::Data>, Fail> {
        let q = self.qty(n);
        // Уступаем поток перед тяжёлым вычислением точки (решение 90) — см. at().
        tokio::task::yield_now().await;
        let work = calc_share::scope(self.share.clone(), self.curve.at(q));
        stage_detail(|| EVAL.into(), work).await
    }

    /// Решение 336: расчёт набора объёмов пачками по par (Curve::batch). Порядок — как дан; уже посчитанные, n = 0 и
    /// повторы пропускаются; сверх предела size.max_evals — не считаются (как at: такие точки — «нет в памяти»).
    /// Пачка с Fail::Stop — следующие пачки не считаются (вызывающий всё равно остановится на этой точке).
    /// Итоги — в памяти (читать at / profit), инвариант бюджета — как у at. Принимает: номера шагов. Ошибок нет.
    pub async fn at_many(&mut self, ns: &[u64]) {
        let mut seen = BTreeSet::new();
        let mut todo: Vec<u64> = ns
            .iter()
            .copied()
            .filter(|n| *n > 0 && !self.map.contains_key(n) && seen.insert(*n))
            .collect();
        todo.truncate(usize::try_from(self.max.saturating_sub(self.done)).unwrap_or(0));
        for chunk in todo.chunks(self.par.max(1)) {
            let qs: Vec<Decimal> = chunk.iter().map(|&n| self.qty(n)).collect();
            self.done += u32::try_from(chunk.len()).unwrap_or(u32::MAX);
            // Уступаем рабочий поток перед пачкой (решение 90, как at).
            tokio::task::yield_now().await;
            let work = calc_share::scope(self.share.clone(), self.curve.batch(qs));
            let rs = stage_detail(|| EVAL_BATCH.into(), work).await;
            let mut stop = false;
            for (&n, r) in chunk.iter().zip(rs) {
                let q = self.qty(n);
                let r = match r {
                    Ok(p) if p.invested > self.budget => {
                        Err(over_budget(q, p.invested, self.budget))
                    }
                    r => r,
                };
                stop |= matches!(r, Err(Fail::Stop(_)));
                self.map.insert(n, r);
            }
            if stop {
                break;
            }
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
