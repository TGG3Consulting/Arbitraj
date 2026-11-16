//! Файл: core/size_scan.rs
//! Что делает: решение 343 (ошибки №18 и №19, ревью t159 Б3, Н6, Н8) — участки допустимости объёма (в логике).
//! Допустимое множество объёмов может быть НЕСВЯЗНЫМ: minNotional и maxNotional продажи проверяются по ХУДШЕЙ цене
//! ордера — цене самого глубокого нужного уровня bids (costs.rs → costs_quote.rs order_limits, gateway/frozen.rs
//! deepest), а сумма «количество × цена глубокого уровня» может убывать с ростом количества. Бисекция границ
//! (size_bounds.rs) предполагает монотонность и может отрезать допустимый участок. Здесь:
//! 1) regions — точные участки допустимости продажи по minNotional / maxNotional из стакана B: на уровне j (накопленные
//!    c_{j−1} < s ≤ c_j, худшая цена p_j) допустимо s ∈ [max(c_{j−1}, minN/p_j), min(c_j, maxN/p_j)]; смежные отрезки
//!    склеиваются (участок — Run); подбору нужны концы СКЛЕЕННЫХ участков (ends) без нуля и без конца стакана (его
//!    находит бисекция по глубине): не больше 2 × (участков), а не по концу на каждый уровень;
//! 2) extra — точки концов участков в объёме покупки: шаг до и после (n, n + 1 — как изломы стаканов, size_points.rs
//!    around: точка между двумя шагами количества), перевод по приходу (аффинен по объёму, как изломы B);
//!    начало участка, в котором лежит наименьший объём бисекции, не считается (его нашла бисекция); выше наибольшего
//!    объёма бисекции — только если следующий объём там не «больше бюджета» при монотонном бюджете
//!    (A — биржа: худшая цена asks не убывает; Extra.budget_monotone);
//! 3) равномерные точки выше наибольшего объёма бисекции — только для немонотонностей без формулы (есть сторона DEX:
//!    исполнимость сплита, бюджет умного маршрута) и при size.scan_above_pct (на сколько % выше; size.grid_points точек).
//!    Ниже наименьшего объёма равномерных точек нет: «слишком мал» немонотонен только из-за minNotional продажи по
//!    худшей цене (п. 1 — точно); минимумы вывода, депозита, minQty и minNotional покупки монотонны;
//! 4) rescue (ревью t159 Н6) — бисекция не нашла ни одного допустимого объёма, а участки по стакану B есть: по одной
//!    точке на участок (середина, объём покупки ≈ объём продажи — приход неизвестен без допустимой точки).
//!
//! Итог подбора — по-прежнему лучшая посчитанная допустимая точка (бюджет держит size_memo.rs в каждой точке).
//! Кто вызывает: core/size_curve.rs (regions), core/size_search.rs (extra, rescue).
//! От чего зависит: core/size_memo.rs, core/size.rs, core/types (PriceLevel), gateway/events.rs (SpotPairState).
//! Настройки: size.scan_above_pct (через Params.ext.above_pct), size.grid_points (через Params).

use rust_decimal::Decimal;

use crate::core::types::PriceLevel;
use crate::gateway::events::SpotPairState;

use super::size::{Curve, Fail, Params};
use super::size_memo::Memo;

/// Участок допустимости продажи: начало и конец (None — до конца стакана: эту границу находит бисекция по глубине).
pub type Run = (Decimal, Option<Decimal>);

/// Что делает: склеенные участки допустимости продажи по minNotional / maxNotional по худшей цене. Принимает: уровни
/// bids от лучшего, правила пары B. Возвращает: участки по возрастанию; пусто — пределов суммы нет. Ошибок нет.
pub fn regions(bids: &[PriceLevel], pair: Option<&SpotPairState>) -> Vec<Run> {
    let Some(p) = pair else { return Vec::new() };
    let (min_n, max_n) = (p.min_notional, p.max_notional.filter(|m| !m.is_zero()));
    if min_n.is_none() && max_n.is_none() {
        return Vec::new();
    }
    let mut runs: Vec<(Decimal, Decimal)> = Vec::new();
    let mut prev = Decimal::ZERO;
    for l in bids {
        let cum = prev + l.qty;
        if l.price > Decimal::ZERO {
            let lo = min_n.map_or(prev, |m| prev.max(m / l.price));
            let hi = max_n.map_or(cum, |m| cum.min(m / l.price));
            if lo <= hi {
                // Ревью t159 Б3: смежный с предыдущим (его конец — начало этого) — один участок.
                match runs.last_mut() {
                    Some(r) if lo <= r.1 => r.1 = r.1.max(hi),
                    _ => runs.push((lo, hi)),
                }
            }
        }
        prev = cum;
    }
    runs.into_iter()
        .map(|(lo, hi)| (lo, (hi < prev).then_some(hi)))
        .collect()
}

/// Что делает: концы участков, нужные подбору: начало > 0 и конец не у конца стакана. Принимает: участки.
/// Возвращает: количества продажи по возрастанию. Ошибок нет.
pub fn ends(runs: &[Run]) -> Vec<Decimal> {
    let mut out: Vec<Decimal> = Vec::new();
    for (lo, hi) in runs {
        if *lo > Decimal::ZERO {
            out.push(*lo);
        }
        out.extend(hi.iter().copied());
    }
    out
}

/// Что делает: п. 2–3 шапки. Принимает: расчёты, параметры, границы бисекции в шагах. Возвращает: допустимые точки с
/// прибылью (номер шага, прибыль). Ошибки: Stop.
pub async fn extra<C: Curve>(
    m: &mut Memo<'_, C>,
    p: &Params,
    lo: u64,
    hi: u64,
) -> Result<Vec<(u64, Decimal)>, Fail> {
    // Выше hi при монотонном бюджете и «больше бюджета» на ближайшем посчитанном выше — недопустимо всё.
    let budget_wall = p.ext.budget_monotone
        && m.map
            .range(hi.saturating_add(1)..)
            .next()
            .is_some_and(|(_, r)| matches!(r, Err(Fail::OverBudget(_))));
    let mut pts: Vec<u64> = Vec::new();
    if !p.ext.sell_regions.is_empty()
        && let (Ok(a), Ok(b)) = (m.at(lo).await, m.at(hi).await)
        && b.arrive > a.arrive
    {
        // Повторное ревью t159 Н-3: начало участка, в котором лежит наименьший объём бисекции, — граница
        // «слишком мал», её нашла бисекция; ниже — только отдельные участки, целиком ниже него.
        let runs: Vec<Run> = p
            .ext
            .sell_regions
            .iter()
            .map(|&(s, e)| {
                let holds_lo = s <= a.arrive && e.is_none_or(|e| e >= a.arrive);
                (if holds_lo { Decimal::ZERO } else { s }, e)
            })
            .collect();
        for s in ends(&runs) {
            let q = a.qty + (s - a.arrive) * (b.qty - a.qty) / (b.arrive - a.arrive);
            if q > Decimal::ZERO
                && let Some(n) = m.units(q)
            {
                pts.extend([n, n.saturating_add(1)]);
            }
        }
    }
    if p.ext.no_formula
        && !budget_wall
        && let Some(pct) = p.ext.above_pct
    {
        let g = u64::from(p.grid_points);
        let span = m.units(m.qty(hi) * pct / Decimal::ONE_HUNDRED).unwrap_or(0);
        pts.extend((1..=g).map(|i| hi.saturating_add(mul_div(span, i, g))));
    }
    pts.retain(|n| *n > 0 && (!budget_wall || *n <= hi));
    pts.sort_unstable();
    pts.dedup();
    if m.par() > 1 {
        m.at_many(&pts).await;
    }
    let mut ok = Vec::new();
    for n in pts {
        if let Some(v) = m.profit(n).await? {
            ok.push((n, v));
        }
    }
    Ok(ok)
}

/// Что делает: п. 4 шапки — по точке на участок продажи (середина; до конца стакана — начало; объём покупки ≈ объём
/// продажи). Принимает: расчёты, участки. Возвращает: допустимые номера шагов по возрастанию. Ошибки: Stop.
pub async fn rescue<C: Curve>(m: &mut Memo<'_, C>, runs: &[Run]) -> Result<Vec<u64>, Fail> {
    let mut pts: Vec<u64> = runs
        .iter()
        .filter_map(|(a, b)| m.units(b.map_or(*a, |b| (*a + b) / Decimal::TWO)))
        .filter(|n| *n > 0)
        .collect();
    pts.sort_unstable();
    pts.dedup();
    if m.par() > 1 {
        m.at_many(&pts).await;
    }
    let mut ok = Vec::new();
    for n in pts {
        if m.profit(n).await?.is_some() {
            ok.push(n);
        }
    }
    Ok(ok)
}

/// span · i / d без переполнения (d > 0). Ошибок нет.
fn mul_div(span: u64, i: u64, d: u64) -> u64 {
    (u128::from(span) * u128::from(i) / u128::from(d.max(1))) as u64
}
