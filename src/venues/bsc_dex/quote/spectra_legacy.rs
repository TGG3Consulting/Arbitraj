//! Выбор меньшего проверенного входа на немонотонных участках legacy Curve в Spectra.
use super::{U256, first_input, full_math};

/// В legacy Curve целочисленный `newton_y` может уменьшить выплату при большем входе.
/// Масштаб проб берём из первой нормировки K0_i = 2·10^18·xp[i]/D самого контракта.
/// Каждая принятая цена проверяется прямой функцией; шаг нормировки задаёт сетку проб.
pub(super) fn crypto_input(
    seed: U256,
    cap: U256,
    wanted: U256,
    data: &crate::core::types::CryptoData,
    i: usize,
    payout: impl Fn(U256) -> Option<U256>,
) -> Option<(U256, U256)> {
    let (mut best, mut out) = first_input(seed, cap, wanted, &payout)?;
    if best == U256::ONE {
        return Some((best, out));
    }
    let scale = if i == 0 {
        U256::from(10u64.pow(18))
    } else {
        *data.price_scale.get(i - 1)?
    };
    let precision = *data.precisions.get(i)?;
    if precision.is_zero() || scale.is_zero() {
        return None;
    }
    let quantum = precision
        .checked_mul(scale)
        .and_then(|v| v.checked_mul(U256::from(2)))
        .map_or(U256::ONE, |den| data.d.div_ceil(den).max(U256::ONE));
    let start = best;
    let window = quantum.saturating_mul(U256::from(4)).min(start - U256::ONE);
    // Предельные объёмы глубины обычно заданы долей резерва, а заявки — целыми
    // разрядами входной валюты. Такие входы проверяем наравне с промежуточными.
    // Дробь восстанавливаем из текущих балансов. Знаменатели и размеры ордеров не задаются.
    let reserve = *data.balances.get(i)?;
    let landmark = reserve_landmark(start, reserve, window);
    for point in [landmark, decimal_landmark(start, window)]
        .into_iter()
        .flatten()
    {
        for x in [point, point.saturating_sub(U256::ONE)] {
            if !x.is_zero()
                && x < best
                && let Some(y) = payout(x).filter(|v| *v >= wanted)
            {
                (best, out) = (x, y);
            }
        }
    }
    let mut nearest_short = None;
    let mut previous = U256::ZERO;
    for k in 1..=40u64 {
        // Входы на единицах ERC-20 и равномерно по окрестности первой нормировки.
        let delta = if k <= 8 {
            U256::from(k)
        } else {
            full_math::mul_div_up(window, U256::from(k - 8), U256::from(32))?
        };
        if delta <= previous || delta >= start {
            continue;
        }
        previous = delta;
        let x = start - delta;
        if x < best && let Some(y) = payout(x) {
            if y >= wanted { (best,out)=(x,y); }
            else if nearest_short.is_none_or(|(_,previous)| y>previous) { nearest_short=Some((x,y)); }
        }
    }
    // Внутри интервала Ньютона два соседних входа могут лежать по разные стороны целевой выплаты.
    // Уточняем окрестности лучшего входа и ближайшей к цели меньшей выплаты.
    let centers=[Some(best), nearest_short.map(|(x,_)| x).filter(|x|*x<best)];
    for center in centers.into_iter().flatten() {
        for distance in [1u8,2,3,4] {
            let delta=U256::from(distance);
            for x in [center.saturating_sub(delta), center.saturating_add(delta)] {
                if !x.is_zero() && x<best && let Some(y)=payout(x).filter(|y|*y>=wanted) {
                    (best,out)=(x,y);
                }
            }
        }
    }
    // Если у найденной точки работает и предшествующий атом, сужаем цену за счёт известных выплат.
    if best < start && best > U256::ONE && payout(best - U256::ONE).is_some_and(|y| y >= wanted) {
        let mut step = U256::ONE;
        let hi = best;
        loop {
            let x = hi.saturating_sub(step).max(U256::ONE);
            if let Some(y) = payout(x).filter(|y| *y >= wanted) {
                (best, out) = (x, y);
                if x == U256::ONE {
                    break;
                }
                step = step.saturating_mul(U256::from(2));
            } else {
                let mut low = x;
                while best - low > U256::ONE {
                    let mid = low + (best - low) / U256::from(2);
                    if let Some(y) = payout(mid).filter(|y| *y >= wanted) {
                        (best, out) = (mid, y);
                    } else {
                        low = mid;
                    }
                }
                break;
            }
        }
    }
    Some((best, out))
}

/// Самый крупный десятичный разряд, округление вниз к которому попадает в исследуемый диапазон.
fn decimal_landmark(point: U256, width: U256) -> Option<U256> {
    let low = point.saturating_sub(width).max(U256::ONE);
    let mut unit = U256::ONE;
    loop {
        let next = unit.checked_mul(U256::from(10))?;
        if point - point % next < low {
            return Some(point - point % unit);
        }
        unit = next;
    }
}

/// Первая подходящая дробь цепного разложения входа относительно текущего резерва.
/// После перевода дроби в атомы её исполнимость всегда перепроверяет сам пул.
fn reserve_landmark(point: U256, reserve: U256, width: U256) -> Option<U256> {
    let (mut n, mut d) = (point, reserve);
    let (mut prev_n, mut cur_n) = (U256::ZERO, U256::ONE);
    let (mut prev_d, mut cur_d) = (U256::ONE, U256::ZERO);
    while !d.is_zero() {
        let (q, r) = n.div_rem(d);
        let num = q.checked_mul(cur_n)?.checked_add(prev_n)?;
        let den = q.checked_mul(cur_d)?.checked_add(prev_d)?;
        if !num.is_zero() {
            let x = full_math::mul_div(reserve, num, den)?;
            if x.abs_diff(point) <= width {
                return Some(x);
            }
        }
        (n, d) = (d, r);
        (prev_n, cur_n) = (cur_n, num);
        (prev_d, cur_d) = (cur_d, den);
    }
    None
}
