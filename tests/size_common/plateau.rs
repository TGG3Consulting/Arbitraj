//! Исследовательская стадия после обычного подбора. Вызывающий владеет одним
//! неизменяемым снимком обеих ног, комиссий и поправок на перевод.
use std::future::Future;
use std::time::Instant;

use arb::core::size::{Curve, Point};
use arb::core::size_memo::Memo;
use rust_decimal::Decimal;

/// Первый исходный шаг, дающий ровно target. forward — дешёвая монотонная
/// функция количества на данном участке, без котировок. Отказ на любом
/// шаге означает, что граница не доказана. Шаг 0 даёт нулевую продажу.
pub async fn first_equal<F, Fut>(
    last: u64,
    target: Decimal,
    deadline: Instant,
    forward: F,
) -> Option<u64>
where
    F: Fn(u64) -> Fut,
    Fut: Future<Output = Option<Decimal>>,
{
    if last == 0 || target <= Decimal::ZERO {
        return None;
    }
    let probe = async |units| {
        if Instant::now() >= deadline {
            return None;
        }
        let value = tokio::time::timeout_at(deadline.into(), forward(units))
            .await
            .ok()??;
        (Instant::now() < deadline).then_some(value)
    };
    let (mut lo, mut hi) = (1, last);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if probe(mid).await? < target {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if probe(lo).await? != target || probe(lo - 1).await? >= target {
        return None;
    }
    Some(lo)
}

/// Обе точки должны пройти денежный и процентный пороги. При двух точных
/// равенствах сохраняется штатный результат и его порядок.
pub fn improves<T>(candidate: &Point<T>, current: &Point<T>) -> bool {
    candidate.profit > current.profit
        || (candidate.profit == current.profit && candidate.invested < current.invested)
}

/// Только для Memo одного зафиксированного сценария. На промахе Memo::at
/// расходует общую квоту и сохраняет также отказы. У Memo боя нет версии
/// входов, поэтому там вызова этой функции нет.
pub async fn consider<C, F, Fut>(
    memo: &mut Memo<'_, C>,
    current: &Point<C::Data>,
    target: Decimal,
    deadline: Instant,
    forward: F,
) -> Option<Point<C::Data>>
where
    C: Curve,
    F: Fn(u64) -> Fut,
    Fut: Future<Output = Option<Decimal>>,
{
    if !memo.gate().passes::<C>(current) {
        return None;
    }
    let edge = first_equal(memo.units(current.qty)?, target, deadline, forward).await?;
    if Instant::now() >= deadline {
        return None;
    }
    let point = match memo.map.get(&edge) {
        Some(result) => result.as_ref().ok()?.clone(),
        None if memo.capped() => return None,
        None => tokio::time::timeout_at(deadline.into(), memo.at(edge))
            .await
            .ok()?
            .ok()?,
    };
    (Instant::now() < deadline && memo.gate().passes::<C>(&point) && improves(&point, current))
        .then_some(point)
}
