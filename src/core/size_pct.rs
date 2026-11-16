//! Файл: core/size_pct.rs
//! Что делает: решение 342 (ошибка №20, ревью t159 Б1) — порог % как ОГРАНИЧЕНИЕ поиска объёма (в логике): цель —
//! наибольшая прибыль P в USDT среди объёмов, проходящих оба порога (T_usd, T_pct). % = P / I · 100, I — база
//! процента (Curve::pct_base: у ядра Calc.invested = объём × средняя цена покупки, строго растёт с объёмом).
//! Запас по порогу % в USDT: g(q) = P(q) − T_pct/100 · I(q); g ≥ 0 ⇔ % ≥ T_pct (I > 0).
//!
//! Шаги (после обычного поиска по USDT, size_search.rs; q* — лучший посчитанный по USDT):
//! 0) q* проходит % — итог прежний, 0 точек. P(q*) < T_usd — 0 точек: у любой посчитанной P ≤ P(q*) < T_usd.
//! 1) Правее q* не ищем (доказано): для посчитанного q > q* P(q) ≤ P(q*) (q* — лучший), I(q) > I(q*), P(q*) ≥ T_usd > 0
//!    (money.min_profit_usd > 0) ⇒ %(q) ≤ P(q*)/I(q) < P(q*)/I(q*) = %(q*) < T_pct. Объём правее q*, проходящий %,
//!    имел бы P > P(q*) — это недобор поиска по USDT (сетка, изломы), не порога %. При P(q*) < 0 (только без T_usd)
//!    правая граница — наибольший посчитанный допустимый объём.
//! 2) Пики g: локальные максимумы g среди посчитанных допустимых точек между a₀ и q* (a₀ — лучшая проходящая %, нет —
//!    весь [1, right]; дальше от q*, чем a₀, прибыль в модели ниже), ещё не проходящие, — тернарный поиск g между
//!    соседями (size_par.rs tern_all, все пики в ногу, точки шага — пачкой at_many). Почему g, а не %: в модели теоремы 1 (docs/notes/t159-…, п. 2.1) P
//!    вогнута, I выпукла (средняя цена покупки не убывает) ⇒ g вогнута при T_pct ≥ 0 — тернарный поиск точен до
//!    ширины остановки; % = P/I лишь квазивогнута. Изломы (уровни, тики) вогнутость P не нарушают; скачки и разрывы
//!    допустимости (участки 343) — каждый посчитанный локальный максимум g уточняется отдельно, как пики USDT.
//! 3) Граница: от лучшей проходящей % точки a к ближайшей посчитанной не проходящей c в сторону q* — бисекция по
//!    признаку «% ≥ порога» (k проб пачкой при size.bisect_probes = k > 1). В модели множество {g ≥ 0} — отрезок
//!    [x₁, x₂] ⊂ [lo, q*], на [argmax g, q*] g не возрастает ⇒ признак монотонен, бисекция находит x₂; на [lo, q*] P не
//!    убывает (вогнута, максимум в q*) ⇒ x₂ — наибольшая прибыль среди проходящих.
//! 4) Итог — лучшая по USDT среди посчитанных проходящих % (size_gate.rs best_pct); запасной (ревью t159 Н2) —
//!    ближайший меньший посчитанный объём, проходящий оба порога (один пересчёт финальной котировкой в profit.rs).
//!
//! Точек сверх обычного поиска: шаг 0 — 0; шаг 2 — на пик не больше 2·size.refine_steps + 3 (в модели пик один);
//! шаг 3 — не больше k·size.bisect_steps и ⌈log_{k+1}((c − a)/ширина)⌉·k; всё — в пределе size.max_evals и мягкого
//! срока, ПОСЛЕ обычного поиска (не вытесняет его). Где модель не держится (немонотонный маршрут DEX, пила
//! округлений): пропуск проходящего участка возможен, только если в нём нет ни посчитанной точки, ни пика g — та же
//! граница, что у поиска по USDT.
//! Кто вызывает: core/size_search.rs.
//! От чего зависит: core/size_memo.rs, core/size_gate.rs, core/size_par.rs, core/size.rs.
//! Настройки: нет своих (пороги — Params.ext; size.refine_steps, size.tolerance_pct, size.bisect_steps,
//! size.bisect_probes — через Params).

use rust_decimal::Decimal;

use super::size::{Curve, Fail, Params, Point};
use super::size_memo::Memo;
use super::size_par::{probes, tern_all};

/// Итог ограниченной задачи: (выбранная точка, запасная меньшая проходящая — в куче, см. size.rs Found.fallback).
pub type Pick<T> = (Point<T>, Option<Box<Point<T>>>);

/// Что делает: шаги 0–4 (см. шапку). Принимает: расчёты (пороги — Memo::gate), параметры, ширину остановки (шагов).
/// Возвращает: Some — итог сдвинут порогом %; None — итог прежний (лучший по USDT проходит %, или проходящих нет, или
/// порога % нет). Ошибки: Stop (расчёт невозможен).
pub async fn best<C: Curve>(
    m: &mut Memo<'_, C>,
    p: &Params,
    tol: u64,
) -> Result<Option<Pick<C::Data>>, Fail> {
    let gate = m.gate();
    let Some(min_pct) = gate.min_pct else {
        return Ok(None);
    };
    let Some(star) = m.best() else {
        return Ok(None);
    };
    if C::pct(&star.data).is_none()
        || gate.pct_ok::<C>(&star)
        || gate.min_usd.is_some_and(|u| star.profit < u)
    {
        return Ok(None);
    }
    let Some(n_star) = m.units(star.qty) else {
        return Ok(None);
    };
    let right = if star.profit >= Decimal::ZERO {
        n_star
    } else {
        m.map
            .iter()
            .rev()
            .find(|(_, r)| r.is_ok())
            .map_or(n_star, |(n, _)| *n)
    };
    let hundred = Decimal::ONE_HUNDRED;
    let g = move |pt: &Point<C::Data>| -> Option<Decimal> {
        C::pct_base(&pt.data).map(|b| pt.profit - min_pct * b / hundred)
    };
    // Шаг 2: пики g между лучшей проходящей и q* (нет проходящей — на всём [1, right]).
    let a0 = m.best_pct(Some(right)).map(|(n, _)| n);
    let span = match a0 {
        Some(a) if a < n_star => (a.saturating_add(1), n_star),
        Some(a) => (n_star, a.saturating_sub(1)),
        None => (1, right),
    };
    let spans = peaks(m, span, &g);
    if !spans.is_empty() {
        tern_all(m, &spans, p.refine_steps, tol, &g).await?;
    }
    // Шаг 3: граница проходящего участка в сторону q*.
    let Some((a, _)) = m.best_pct(Some(right)) else {
        return Ok(None);
    };
    edge(m, a, n_star, p, tol).await?;
    let Some((nb, b)) = m.best_pct(None) else {
        return Ok(None);
    };
    // Шаг 4: запасной — ближайший меньший посчитанный, проходящий оба порога.
    let fallback = m.map.range(..nb).rev().find_map(|(_, r)| match r {
        Ok(q) if gate.passes::<C>(q) => Some(Box::new(q.clone())),
        _ => None,
    });
    Ok(Some((b, fallback)))
}

/// Что делает: отрезки уточнения пиков g — локальные максимумы g среди посчитанных допустимых точек, лежащие в
/// [from, to] и ещё не проходящие (g < 0); отрезок — соседние допустимые точки (соседи — и вне [from, to]).
/// Принимает: расчёты, (from, to), значение g. Возвращает: отрезки [a, b]. Ошибок нет.
fn peaks<C: Curve, F>(m: &Memo<'_, C>, (from, to): (u64, u64), g: &F) -> Vec<(u64, u64)>
where
    F: Fn(&Point<C::Data>) -> Option<Decimal>,
{
    let pts: Vec<(u64, Decimal)> = m
        .map
        .iter()
        .filter_map(|(n, r)| r.as_ref().ok().and_then(g).map(|v| (*n, v)))
        .collect();
    let mut spans = Vec::new();
    for i in 0..pts.len() {
        let (n, v) = pts[i];
        let left_ok = i == 0 || v >= pts[i - 1].1;
        let right_ok = i + 1 == pts.len() || v >= pts[i + 1].1;
        if !(left_ok && right_ok) || v >= Decimal::ZERO || !(from..=to).contains(&n) {
            continue;
        }
        let a = if i == 0 { n } else { pts[i - 1].0 };
        let b = if i + 1 == pts.len() { n } else { pts[i + 1].0 };
        spans.push((a, b));
    }
    spans
}

/// Что делает: шаг 3 — бисекция по признаку «% ≥ порога» от проходящей a к ближайшей посчитанной не проходящей в
/// сторону q* (при P(q*) ≥ 0 это правее a; ниже нуля — в любую сторону, где q*), до ширины остановки или
/// size.bisect_steps шагов (k проб пачкой). Принимает: расчёты, a, n* (номер q*), параметры, ширину. Возвращает: ().
/// Ошибки: Stop.
async fn edge<C: Curve>(
    m: &mut Memo<'_, C>,
    a: u64,
    star: u64,
    p: &Params,
    tol: u64,
) -> Result<(), Fail> {
    if a == star {
        return Ok(());
    }
    let gate = m.gate();
    let fails = |r: &Result<Point<C::Data>, Fail>| !matches!(r, Ok(q) if gate.pct_ok::<C>(q));
    let near = if a < star {
        m.map.range(a + 1..=star).find(|e| fails(e.1))
    } else {
        m.map.range(star..a).rev().find(|e| fails(e.1))
    };
    let Some(mut fail) = near.map(|(n, _)| *n) else {
        return Ok(());
    };
    let (mut pass, mut left) = (a, p.bisect_steps);
    'outer: while pass.abs_diff(fail) > tol && left > 0 && !m.capped() {
        left -= 1;
        let mut pts = probes(pass.min(fail), pass.max(fail), p.bisect_probes);
        if pts.is_empty() {
            break;
        }
        m.at_many(&pts).await;
        // Пробы — от проходящей стороны к непроходящей.
        if pass > fail {
            pts.reverse();
        }
        for n in pts {
            // Не посчитана (предел или срок) — дальше не уточняем.
            let Some(r) = m.map.get(&n) else {
                break 'outer;
            };
            if let Err(Fail::Stop(e)) = r {
                return Err(Fail::Stop(e.clone()));
            }
            if fails(r) {
                fail = n;
                break;
            }
            pass = n;
        }
    }
    Ok(())
}
