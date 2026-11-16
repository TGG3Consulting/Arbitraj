//! Файл: core/size_scan.rs
//! Что делает: решение 343 (задача t159, ошибки №18 и №19) — участки допустимости объёма. Допустимое множество
//! объёмов может быть НЕСВЯЗНЫМ: minNotional и maxNotional продажи проверяются по ХУДШЕЙ цене ордера — цене самого
//! глубокого нужного уровня bids (costs.rs → costs_quote.rs order_limits, gateway/frozen.rs deepest), а сумма
//! «количество × цена глубокого уровня» может убывать с ростом количества (следующий уровень сильно ниже); исполнимость
//! сплита DEX и бюджет по худшей цене умного маршрута тоже немонотонны. Бисекция границ (size_bounds.rs) предполагает
//! монотонность и может отрезать допустимый участок. Здесь (size.feasible_scan = да):
//! 1) regions — точные границы участков допустимости по minNotional / maxNotional из стакана B (в монетах продажи):
//!    на уровне j (накопленные c_{j−1} < s ≤ c_j, худшая цена p_j) допустимо s ∈ [max(c_{j−1}, minN/p_j), min(c_j,
//!    maxN/p_j)]; концы участков — точки подбора;
//! 2) extra — точки вне отрезка бисекции: концы участков (через приход, как изломы B в size_points.rs) и
//!    size.grid_points равномерных точек ниже наименьшего и выше наибольшего объёма (до удвоенного наибольшего из
//!    оценки бюджета и границы) — для немонотонностей без формулы (исполнимость сплита DEX).
//!
//! Итог подбора — по-прежнему лучшая посчитанная допустимая точка (бюджет держит size_memo.rs в каждой точке).
//! Кто вызывает: core/size_curve.rs (regions), core/size_search.rs (extra).
//! От чего зависит: core/size_memo.rs, core/size.rs, core/types (PriceLevel), gateway/events.rs (SpotPairState).
//! Настройки: size.feasible_scan (через Params.ext.scan), size.grid_points (через Params).

use rust_decimal::Decimal;

use crate::core::types::PriceLevel;
use crate::gateway::events::SpotPairState;

use super::size::{Curve, Fail, Params};
use super::size_memo::Memo;

/// Что делает: концы участков допустимости продажи по minNotional / maxNotional по худшей цене. Принимает: уровни
/// bids от лучшего (как у стакана адаптера), правила пары B. Возвращает: количества продажи — начало и конец каждого
/// допустимого участка внутри уровня (по возрастанию, без повторов); пусто — пределов суммы нет. Ошибок нет.
pub fn regions(bids: &[PriceLevel], pair: Option<&SpotPairState>) -> Vec<Decimal> {
    let Some(p) = pair else { return Vec::new() };
    let (min_n, max_n) = (p.min_notional, p.max_notional.filter(|m| !m.is_zero()));
    if min_n.is_none() && max_n.is_none() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut prev = Decimal::ZERO;
    for l in bids {
        let cum = prev + l.qty;
        if l.price > Decimal::ZERO {
            let lo = min_n.map_or(prev, |m| prev.max(m / l.price));
            let hi = max_n.map_or(cum, |m| cum.min(m / l.price));
            if lo <= hi {
                out.push(lo);
                out.push(hi);
            }
        }
        prev = cum;
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Что делает: точки вне отрезка бисекции (см. шапку). Принимает: расчёты, параметры, границы бисекции в шагах,
/// оценку наибольшего объёма в шагах. Возвращает: допустимые точки с прибылью (номер шага, прибыль). Ошибки: Stop.
pub async fn extra<C: Curve>(
    m: &mut Memo<'_, C>,
    p: &Params,
    lo: u64,
    hi: u64,
    guess: u64,
) -> Result<Vec<(u64, Decimal)>, Fail> {
    let mut pts: Vec<u64> = Vec::new();
    // Концы участков продажи → объём покупки по приходу (приход от объёма аффинен — как изломы B, size_points.rs).
    if !p.ext.sell_regions.is_empty()
        && let (Ok(a), Ok(b)) = (m.at(lo).await, m.at(hi).await)
        && b.arrive > a.arrive
    {
        for s in &p.ext.sell_regions {
            let q = a.qty + (*s - a.arrive) * (b.qty - a.qty) / (b.arrive - a.arrive);
            if q > Decimal::ZERO
                && let Some(n) = m.units(q)
            {
                pts.extend([n.saturating_sub(1), n, n.saturating_add(1)]);
            }
        }
    }
    // Равномерные точки ниже наименьшего и выше наибольшего объёма бисекции.
    let g = u64::from(p.grid_points).max(1);
    let top = hi.max(guess).saturating_mul(2);
    for i in 1..=g {
        pts.push(mul_div(lo, i, g + 1));
        pts.push(hi + mul_div(top.saturating_sub(hi), i, g + 1));
    }
    pts.retain(|n| *n > 0);
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

/// span · i / d без переполнения. Ошибок нет.
fn mul_div(span: u64, i: u64, d: u64) -> u64 {
    (u128::from(span) * u128::from(i) / u128::from(d)) as u64
}
