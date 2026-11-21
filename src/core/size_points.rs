//! Файл: core/size_points.rs
//! Что делает: точки подбора объёма (решение 66, п. 2 size_search.rs): обе границы, шаг до и после каждого излома
//! стакана A (накопленные объёмы уровней asks) и B (bids, пересчёт по приходу), size.grid_points равномерных точек;
//! решение 336 — пачками при size.parallel_points > 1; решение 337 — при поле раннего выхода сначала границы и сетка.
//! Вынесено из size_search.rs без изменений логики (ревью t157 Н5: файл перерос 200 строк).
//! Кто вызывает: core/size_search.rs.
//! От чего зависит: core/size_memo.rs, core/size_batch.rs, core/size_early.rs, core/size.rs.
//! Настройки: size.grid_points (через Params).

use rust_decimal::Decimal;

use super::size::{Curve, Fail, Params};
use super::size_early::below;
use super::size_memo::Memo;

/// Точки подбора: обе границы, шаг до и после каждого излома стакана A и B, size.grid_points равномерных точек.
/// Принимает: расчёты, параметры, границы в шагах. Возвращает: допустимые точки с прибылью по возрастанию объёма и
/// признак раннего выхода (решение 337: пол задан и после границ и сетки лучшая прибыль ниже — изломы не считаются).
/// Ошибки: Stop.
pub async fn points<C: Curve>(
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
