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
//! Все объёмы кратны шагу количества (округление вниз). Вложение ≤ бюджета держит size_memo.rs.
//! Кто вызывает: core/profit.rs, тесты (tests/core_size_optimum.rs — записанные стакан и пул).
//! От чего зависит: core/size.rs, core/size_memo.rs, core/size_bounds.rs.
//! Настройки: size.bisect_steps, size.grid_points, size.refine_steps, size.tolerance_pct, size.max_evals (через Params).

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use super::size::{Curve, Fail, Found, Params, SizeChoice};
use super::size_bounds::{lower, upper};
use super::size_memo::Memo;

/// Поиск. Принимает: кривую, параметры. Возвращает: лучшую точку и сводку (why пустой — текст пишет profit.rs).
/// Ошибки: Stop — расчёт невозможен; TooSmall / TooLarge — ни один объём в пределах бюджета не подходит.
pub async fn search<C: Curve>(curve: &mut C, p: &Params) -> Result<Found<C::Data>, Fail> {
    if p.step <= Decimal::ZERO {
        return Err(Fail::Stop(format!(
            "шаг количества {} не больше нуля",
            p.step
        )));
    }
    let mut m = Memo::new(curve, p.step, p.budget, p.max_evals);
    let guess = m
        .units(p.guess.max(Decimal::ZERO))
        .ok_or_else(|| Fail::Stop(format!("объём {} слишком велик", p.guess)))?;
    let (hi, max_exact) = upper(&mut m, guess, p.bisect_steps).await?;
    let lo = lower(&mut m, hi, p.bisect_steps).await?;
    let mut pts = vec![lo, hi];
    for b in &p.buy_breaks {
        around(&m, *b, lo, hi, &mut pts);
    }
    // Изломы B — по приходу; приход от объёма линейный (затраты в монете: постоянные и в долях), пересчёт — по
    // двум границам. Неточность пересчёта закрывают равномерные точки и уточнение.
    if let (Ok(a), Ok(b)) = (m.at(lo).await, m.at(hi).await)
        && b.arrive > a.arrive
    {
        for s in &p.sell_breaks {
            let q = a.qty + (*s - a.arrive) * (b.qty - a.qty) / (b.arrive - a.arrive);
            around(&m, q, lo, hi, &mut pts);
        }
    }
    let span = hi - lo;
    let g = u64::from(p.grid_points);
    for i in 1..=g {
        pts.push(lo + mul_div(span, i, g + 1));
    }
    pts.sort_unstable();
    pts.dedup();
    let mut ok: Vec<(u64, Decimal)> = Vec::new();
    for n in pts {
        if let Some(v) = m.profit(n).await? {
            ok.push((n, v));
        }
    }
    let tol = tolerance(hi, p.tolerance_pct);
    let mut peaks = 0u32;
    for i in 0..ok.len() {
        let left_ok = i == 0 || ok[i].1 >= ok[i - 1].1;
        let right_ok = i + 1 == ok.len() || ok[i].1 >= ok[i + 1].1;
        if !(left_ok && right_ok) || m.capped() {
            continue;
        }
        peaks += 1;
        let a = if i == 0 { ok[i].0 } else { ok[i - 1].0 };
        let b = if i + 1 == ok.len() {
            ok[i].0
        } else {
            ok[i + 1].0
        };
        refine(&mut m, a, b, p.refine_steps, tol).await?;
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
    Ok(Found { best, choice })
}

/// Тернарный поиск максимума на [a, b] (целые шаги). Принимает: расчёты, концы, предел шагов, ширину остановки.
/// Ошибки: Stop. Недопустимый объём внутри считается хуже любого допустимого.
async fn refine<C: Curve>(
    m: &mut Memo<'_, C>,
    a: u64,
    b: u64,
    steps: u32,
    tol: u64,
) -> Result<(), Fail> {
    let (mut lo, mut hi, mut left) = (a, b, steps);
    while hi - lo > tol.max(2) && left > 0 {
        left -= 1;
        let m1 = lo + (hi - lo) / 3;
        let m2 = hi - (hi - lo) / 3;
        let (f1, f2) = (m.profit(m1).await?, m.profit(m2).await?);
        if f1 < f2 {
            lo = m1;
        } else {
            hi = m2;
        }
    }
    if hi - lo <= 2 {
        for n in lo..=hi {
            m.profit(n).await?;
        }
    } else {
        m.profit(lo + (hi - lo) / 2).await?;
    }
    Ok(())
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

/// Ширина остановки уточнения в шагах: tolerance_pct % от наибольшего объёма, не меньше одного шага. Ошибок нет.
fn tolerance(hi: u64, pct: Decimal) -> u64 {
    (Decimal::from(hi) * pct / Decimal::ONE_HUNDRED)
        .floor()
        .to_u64()
        .unwrap_or(0)
        .max(1)
}
