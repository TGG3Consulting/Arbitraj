//! Файл: core/deal_time.rs
//! Что делает: решение 245 (часть Б) — фактическая длительность наших сделок из учёта ядра для сравнения с окном
//! максимального объёма: дополнение п. 2 (Тигран 09.11: согласовано) — от РЕШЕНИЯ о сделке (Deal::decided_at — время
//! расчёта кандидата, прошедшего пороги прибыли, executor.rs), а не от открытия; у сделок без этого поля (до
//! дополнения, цикл DEX ↔ DEX) — от открытия сделки (Deal::started_at — после предпроверки), как раньше: момента
//! расчёта кандидата в их учёте нет (plan.at — время расчёта предпроверки, а после подгонки под остаток (refit.rs) —
//! подгонки, позже открытия; поэтому не берётся); сколько таких — в Stat.from_open. До исполнения последней продажи
//! (наибольший TradeResult::finished_at среди sells). Берутся
//! только исполненные сделки: шаг Closed, покупка и хотя бы одна продажа есть (Halted, «Беру на себя», без продажи — нет).
//! Медиана и p75 — по паре бирж (buy_on → sell_on); сделок пары меньше uncapped.deal_time_min_n — по бирже-источнику
//! (buy_on, любая площадка продажи); и там меньше — «нет данных». Квантиль — линейная интерполяция между соседними
//! по порядку значениями (q·(n−1)), как у numpy по умолчанию.
//! Кто вызывает: core/maxwin_view.rs.
//! От чего зависит: deal.rs, core/types.
//! Настройки: нет (минимум сделок передаёт вызывающий из uncapped.deal_time_min_n).

use std::collections::HashMap;

use serde::Serialize;

use crate::core::types::VenueId;

use super::deal::{Deal, Step};

/// Медиана и p75 длительности сделок, мс; сколько сделок и по чему: «pair» — пара бирж, «source» — биржа-источник.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stat {
    pub median_ms: i64,
    pub p75_ms: i64,
    pub n: usize,
    pub by: &'static str,
    /// Дополнение п. 2: из n — сколько сделок посчитано от открытия (момента решения в учёте нет — до дополнения).
    pub from_open: usize,
}

/// Длительности исполненных сделок по паре бирж и по бирже-источнику, мс, по возрастанию; сколько из них от открытия.
#[derive(Debug, Default, Clone)]
pub struct DealTimes {
    pair: HashMap<(VenueId, VenueId), Vec<i64>>,
    source: HashMap<VenueId, Vec<i64>>,
    pair_open: HashMap<(VenueId, VenueId), usize>,
    source_open: HashMap<VenueId, usize>,
}

impl DealTimes {
    /// Что делает: длительности по сделкам учёта (см. шапку). Принимает: сделки. Возвращает: DealTimes. Ошибок нет.
    pub fn of<'a>(deals: impl Iterator<Item = &'a Deal>) -> DealTimes {
        let mut t = DealTimes::default();
        for d in deals {
            let Some(ms) = duration_ms(d) else { continue };
            let (a, b) = (d.direction.buy_on.clone(), d.direction.sell_on.clone());
            if d.decided_at.is_none() {
                *t.source_open.entry(a.clone()).or_default() += 1;
                *t.pair_open.entry((a.clone(), b.clone())).or_default() += 1;
            }
            t.source.entry(a.clone()).or_default().push(ms);
            t.pair.entry((a, b)).or_default().push(ms);
        }
        for v in t.pair.values_mut().chain(t.source.values_mut()) {
            v.sort_unstable();
        }
        t
    }

    /// Что делает: медиана и p75 для направления (см. шапку). Принимает: площадку покупки и продажи, минимум сделок.
    /// Возвращает: Stat. Ошибки: текст «нет данных: …» — сделок меньше минимума и у пары, и у источника.
    pub fn stat(&self, buy: &VenueId, sell: &VenueId, min_n: u64) -> Result<Stat, String> {
        let enough = |v: &&Vec<i64>| v.len() as u64 >= min_n.max(1);
        let key = (buy.clone(), sell.clone());
        let pair = self.pair.get(&key);
        if let Some(v) = pair.filter(enough) {
            let open = self.pair_open.get(&key).copied().unwrap_or(0);
            return Ok(stat(v, "pair", open));
        }
        if let Some(v) = self.source.get(buy).filter(enough) {
            let open = self.source_open.get(buy).copied().unwrap_or(0);
            return Ok(stat(v, "source", open));
        }
        let (np, ns) = (
            pair.map_or(0, Vec::len),
            self.source.get(buy).map_or(0, Vec::len),
        );
        Err(format!(
            "нет данных: исполненных сделок {} → {} — {np}, с {} — {ns}, нужно не меньше {min_n} (uncapped.deal_time_min_n)",
            buy.0, sell.0, buy.0
        ))
    }
}

/// Длительность исполненной сделки, мс: от решения (decided_at), без него — от открытия (started_at); None — сделка
/// не исполнена (см. шапку). Ошибок нет.
pub fn duration_ms(d: &Deal) -> Option<i64> {
    if d.step != Step::Closed || d.buy.is_none() {
        return None;
    }
    let end = d.sells.iter().map(|s| s.finished_at).max()?;
    let from = d.decided_at.unwrap_or(d.started_at);
    Some((end - from).num_milliseconds().max(0))
}

/// Медиана и p75 по возрастающему ряду; сколько сделок от открытия. Ошибок нет (ряд не пуст — проверяет вызывающий).
fn stat(v: &[i64], by: &'static str, from_open: usize) -> Stat {
    Stat {
        median_ms: quantile(v, 0.5),
        p75_ms: quantile(v, 0.75),
        n: v.len(),
        by,
        from_open,
    }
}

/// Квантиль q по возрастающему ряду: линейная интерполяция между значениями с номерами ⌊q·(n−1)⌋ и ⌈q·(n−1)⌉,
/// округление до мс. Пустой ряд — 0. Ошибок нет.
pub fn quantile(v: &[i64], q: f64) -> i64 {
    let Some(last) = v.len().checked_sub(1) else {
        return 0;
    };
    let pos = q * last as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    let (a, b) = (v[lo] as f64, v[hi.min(last)] as f64);
    (a + (b - a) * (pos - lo as f64)).round() as i64
}
