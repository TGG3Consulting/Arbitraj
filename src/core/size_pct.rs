//! Файл: core/size_pct.rs
//! Что делает: решение 342 (задача t159, ошибка №20) — порог % как ОГРАНИЧЕНИЕ поиска объёма (size.pct_constraint):
//! цель — наибольшая прибыль в USDT среди объёмов, у которых прибыль в % от вложенного не ниже порога направления.
//! Раньше (profit.rs) поиск брал оптимум по USDT, и если на нём % ниже порога — кандидат отклонялся целиком, хотя
//! меньший объём проходил оба порога («не ходить объёмом 5000 USDT за 0,001 USDT» — назначение порога %, Тигран).
//! Как: после обычного поиска (size_search.rs) берётся лучшая по USDT среди посчитанных точек, проходящих %
//! (size_memo.rs best_pct), и её пик ограниченной задачи уточняется тернарным поиском между соседними посчитанными
//! точками (точка, не проходящая %, хуже любой проходящей — как недопустимый объём в size_refine.rs).
//! Порог USDT отдельно не нужен: если лучшая проходящая % ниже порога USDT — ни одна проходящая % его не проходит
//! среди посчитанных; финальная проверка обоих порогов — как раньше (profit.rs).
//! Кто вызывает: core/size_search.rs.
//! От чего зависит: core/size_memo.rs, core/size.rs.
//! Настройки: size.pct_constraint (через Params.ext.min_pct), size.refine_steps, size.tolerance_pct (через Params).

use rust_decimal::Decimal;

use super::size::{Curve, Fail, Point};
use super::size_memo::Memo;

/// Что делает: лучшая точка ограниченной задачи «прибыль в USDT при проценте ≥ порога» с уточнением её пика.
/// Принимает: расчёты, порог %, предел шагов уточнения, ширину остановки (шагов). Возвращает: точку; None — ни одна
/// посчитанная точка не проходит порог %. Ошибки: Stop (расчёт невозможен).
pub async fn best<C: Curve>(
    m: &mut Memo<'_, C>,
    min_pct: Decimal,
    steps: u32,
    tol: u64,
) -> Result<Option<Point<C::Data>>, Fail> {
    let Some(first) = m.best_pct(min_pct) else {
        return Ok(None);
    };
    let Some(n) = m.units(first.qty) else {
        return Ok(Some(first));
    };
    // Соседи по посчитанным точкам (любым, в том числе недопустимым) — отрезок уточнения пика.
    let a = m.map.range(..n).next_back().map_or(n, |(k, _)| *k);
    let b = m.map.range(n + 1..).next().map_or(n, |(k, _)| *k);
    let (mut lo, mut hi, mut left) = (a, b, steps);
    while hi - lo > tol.max(2) && left > 0 && !m.capped() {
        left -= 1;
        let m1 = lo + (hi - lo) / 3;
        let m2 = hi - (hi - lo) / 3;
        let (f1, f2) = (value(m, m1, min_pct).await?, value(m, m2, min_pct).await?);
        if f1 < f2 {
            lo = m1;
        } else {
            hi = m2;
        }
    }
    if hi - lo <= 2 {
        for k in lo..=hi {
            value(m, k, min_pct).await?;
        }
    }
    Ok(m.best_pct(min_pct).or(Some(first)))
}

/// Значение ограниченной задачи в точке: прибыль, если объём допустим и процент ≥ порога; иначе None (хуже любой
/// допустимой — Option: None < Some). Ошибки: Stop.
async fn value<C: Curve>(
    m: &mut Memo<'_, C>,
    n: u64,
    min_pct: Decimal,
) -> Result<Option<Decimal>, Fail> {
    if n == 0 || (!m.map.contains_key(&n) && m.capped()) {
        return Ok(None);
    }
    match m.at(n).await {
        Ok(p) => Ok(C::pct(&p.data).filter(|v| *v >= min_pct).map(|_| p.profit)),
        Err(Fail::Stop(e)) => Err(Fail::Stop(e)),
        Err(_) => Ok(None),
    }
}
