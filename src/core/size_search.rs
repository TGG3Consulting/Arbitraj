//! Файл: core/size_search.rs
//! Что делает: поиск объёма с наибольшей прибылью в долларах после всех затрат на отрезке [минимум, максимум]
//! (решение 66). Прибыль от объёма кусочно-гладкая: изломы — на уровнях стаканов (A — по объёму покупки, B — по
//! приходу), тиках пулов и порогах минимумов; локальный максимум не обязательно глобальный. Поэтому:
//! 1) границы — size_bounds.rs (максимум — точно по бюджету, не «бюджет / лучшая цена»);
//! 2) точки: обе границы, каждый излом стакана (шаг до и после него), size.grid_points равномерных точек —
//!    они ловят изломы, которых ядро не видит (тики пулов, смена маршрута DEX);
//! 3) каждый локальный максимум среди точек (все, а не первый) уточняется тернарным поиском между соседями —
//!    не больше size.refine_steps шагов, до ширины size.tolerance_pct от максимума (не точнее шага количества);
//! 4) итог — лучшая из всех посчитанных точек; при равной прибыли — меньший объём.
//!
//! Глобальный максимум — эвристика: изломы, которых ядро не видит, ловит только сетка; у умного маршрута DEX
//! (quote/pick.rs — больший из двух путей) вложение от объёма может быть немонотонным, тогда бисекция границы бюджета
//! может найти не самый большой объём — но бюджет проверяется в каждой точке (size_memo.rs), он не нарушается.
//! Все расчёты — не больше size.max_evals: исчерпан на границах — отказ, дальше — лучший из посчитанных.
//!
//! Решение 265: границы, точки и уточнение — части подробного замера этапа «подбор объёма» (calc_detail.rs, только при log.step_timing).
//! Все объёмы кратны шагу количества (округление вниз). Вложение ≤ бюджета держит size_memo.rs.
//! Решение 336 (задача t157): при size.parallel_points > 1 точки п. 2 считаются пачками (size_memo.rs at_many) — набор
//! точек тот же, итог тот же; уточнение п. 3 — у всех пиков в ногу (size_par.rs refine_all) — те же точки, итог тот же,
//! пока не исчерпан size.max_evals; границы — k-ичным поиском при size.bisect_probes > 1 (size_bounds.rs).
//! Не заданы — ровно как до 336 (по одной точке).
//! Решение 337: Params.early_floor задан — сначала границы и сетка; лучшая прибыль ниже пола (size_early.rs) — изломы
//! и уточнение не считаются, Found.early (вызывающий отклоняет без финальной котировки). Не задан — как до 337.
//! Кто вызывает: core/profit.rs, core/cycle_plan.rs, core/size_uncapped.rs, core/size_max.rs, тесты
//! (tests/core_size_optimum.rs, tests/core_size_parallel.rs — записанные стакан и пул).
//! От чего зависит: core/size.rs, core/size_memo.rs, core/size_bounds.rs, core/size_par.rs, core/calc_detail.rs.
//! Настройки: size.bisect_steps, size.grid_points, size.refine_steps, size.tolerance_pct, size.max_evals,
//! size.parallel_points, size.bisect_probes (через Params).

use rust_decimal::Decimal;

use super::calc_detail::stage_detail;
use super::size::{Curve, Fail, Found, Params, SizeChoice};
use super::size_bounds::{lower, upper};
use super::size_early::below;
use super::size_memo::Memo;
use super::size_par::refine_all;
use super::size_refine::{refine, tolerance};

/// Решение 265: имена частей замера подбора (calc_detail.rs) — их же ищет текст «Время по шагам» (calc_steps.rs).
pub const BOUNDS: &str = "границы (бисекция)";
pub const POINTS: &str = "точки (изломы и сетка)";
pub const REFINE: &str = "уточнение";

/// Поиск. Принимает: кривую, параметры. Возвращает: лучшую точку и сводку (why пустой — текст пишет profit.rs).
/// Ошибки: Stop — расчёт невозможен; TooSmall / TooLarge — ни один объём в пределах бюджета не подходит.
pub async fn search<C: Curve>(curve: &mut C, p: &Params) -> Result<Found<C::Data>, Fail> {
    if p.step <= Decimal::ZERO {
        return Err(Fail::Stop(format!(
            "шаг количества {} не больше нуля",
            p.step
        )));
    }
    let mut m = Memo::new(curve, p.step, p.budget, p.max_evals).with_par(p.par_points);
    search_in(&mut m, p).await
}

/// Тот же поиск на готовых расчётах (решение 74: поиск без бюджета продолжает на них бисекцию, не считая точки
/// заново). Принимает: расчёты (шаг и бюджет — из p), параметры. Возвращает и ошибки: как search.
pub async fn search_in<C: Curve>(m: &mut Memo<'_, C>, p: &Params) -> Result<Found<C::Data>, Fail> {
    let guess = m
        .units(p.guess.max(Decimal::ZERO))
        .ok_or_else(|| Fail::Stop(format!("объём {} слишком велик", p.guess)))?;
    // Решение 265: этапы подбора — части подробного замера (calc_detail.rs; иначе — просто работа).
    let (hi, max_exact, lo) = stage_detail(|| BOUNDS.into(), async {
        let (hi, max_exact) = upper(m, guess, p.bisect_steps, p.bisect_probes).await?;
        let lo = lower(m, hi, p.bisect_steps, p.bisect_probes).await?;
        Ok::<_, Fail>((hi, max_exact, lo))
    })
    .await?;
    // Решение 337: при поле раннего выхода сначала границы и сетка, ниже пола — без изломов и уточнения.
    let (ok, early) = stage_detail(|| POINTS.into(), points(m, p, lo, hi)).await?;
    let early = early || below(m, p.early_floor);
    let tol = tolerance(hi, p.tolerance_pct);
    let mut peaks = 0u32;
    // Решение 336: параллельные точки — все пики уточняются в ногу (те же точки, что по очереди).
    let par = m.par() > 1;
    let mut spans: Vec<(u64, u64)> = Vec::new();
    for i in 0..ok.len() {
        let left_ok = i == 0 || ok[i].1 >= ok[i - 1].1;
        let right_ok = i + 1 == ok.len() || ok[i].1 >= ok[i + 1].1;
        if !(left_ok && right_ok) || m.capped() || early {
            continue;
        }
        peaks += 1;
        let a = if i == 0 { ok[i].0 } else { ok[i - 1].0 };
        let b = if i + 1 == ok.len() {
            ok[i].0
        } else {
            ok[i + 1].0
        };
        if par {
            spans.push((a, b));
            continue;
        }
        stage_detail(|| REFINE.into(), refine(m, a, b, p.refine_steps, tol)).await?;
    }
    if !spans.is_empty() {
        let all = refine_all(m, &spans, p.refine_steps, tol);
        stage_detail(|| REFINE.into(), all).await?;
    }
    let best = m
        .best()
        .ok_or_else(|| Fail::Stop("ни один объём не посчитан".into()))?;
    // Точка hi посчитана при поиске границы (в памяти) — предел расчётов её не трогает.
    let top = m.at(hi).await?;
    let choice = SizeChoice {
        qty: best.qty,
        profit: best.profit,
        min_qty: m.qty(lo),
        max_qty: top.qty,
        budget: p.budget,
        at_budget_profit: top.profit,
        max_exact,
        evals: m.evals(),
        peaks,
        capped: m.capped(),
        why: String::new(),
    };
    Ok(Found {
        best,
        choice,
        early,
    })
}

/// Точки подбора: обе границы, шаг до и после каждого излома стакана A и B, size.grid_points равномерных точек.
/// Принимает: расчёты, параметры, границы в шагах. Возвращает: допустимые точки с прибылью по возрастанию объёма и
/// признак раннего выхода (решение 337: пол задан и после границ и сетки лучшая прибыль ниже — изломы не считаются).
/// Ошибки: Stop.
async fn points<C: Curve>(
    m: &mut Memo<'_, C>,
    p: &Params,
    lo: u64,
    hi: u64,
) -> Result<(Vec<(u64, Decimal)>, bool), Fail> {
    // Решение 337: сетка — первой (тот же набор точек; порядок меняется только при заданном поле).
    if p.early_floor.is_some() {
        let first = grid(lo, hi, p.grid_points);
        if m.par() > 1 {
            m.at_many(&first).await;
        }
        let mut ok = Vec::new();
        for n in first {
            if let Some(v) = m.profit(n).await? {
                ok.push((n, v));
            }
        }
        if below(m, p.early_floor) {
            return Ok((ok, true));
        }
    }
    let mut pts = vec![lo, hi];
    for b in &p.buy_breaks {
        around(m, *b, lo, hi, &mut pts);
    }
    // Изломы B — по приходу; приход от объёма линейный (затраты в монете: постоянные и в долях), пересчёт — по
    // двум границам. Неточность пересчёта закрывают равномерные точки и уточнение.
    if let (Ok(a), Ok(b)) = (m.at(lo).await, m.at(hi).await)
        && b.arrive > a.arrive
    {
        for s in &p.sell_breaks {
            let q = a.qty + (*s - a.arrive) * (b.qty - a.qty) / (b.arrive - a.arrive);
            around(m, q, lo, hi, &mut pts);
        }
    }
    pts.extend(grid(lo, hi, p.grid_points));
    pts.sort_unstable();
    pts.dedup();
    // Решение 336: точки не зависят друг от друга — пачками (итоги в памяти; ниже — тот же проход по порядку).
    if m.par() > 1 {
        m.at_many(&pts).await;
    }
    let mut ok: Vec<(u64, Decimal)> = Vec::new();
    for n in pts {
        if let Some(v) = m.profit(n).await? {
            ok.push((n, v));
        }
    }
    Ok((ok, false))
}

/// Обе границы и size.grid_points равномерных точек между ними, по возрастанию без повторов. Ошибок нет.
fn grid(lo: u64, hi: u64, points: u32) -> Vec<u64> {
    let (span, g) = (hi - lo, u64::from(points));
    let mut v = vec![lo, hi];
    v.extend((1..=g).map(|i| lo + mul_div(span, i, g + 1)));
    v.sort_unstable();
    v.dedup();
    v
}

/// Добавляет шаг до и после излома q, если они на отрезке [lo, hi]. Ошибок нет.
fn around<C: Curve>(m: &Memo<'_, C>, q: Decimal, lo: u64, hi: u64, pts: &mut Vec<u64>) {
    if q <= Decimal::ZERO {
        return;
    }
    if let Some(n) = m.units(q) {
        for k in [n, n.saturating_add(1)] {
            if (lo..=hi).contains(&k) {
                pts.push(k);
            }
        }
    }
}

/// span · i / d без переполнения. Ошибок нет.
fn mul_div(span: u64, i: u64, d: u64) -> u64 {
    (u128::from(span) * u128::from(i) / u128::from(d)) as u64
}
