//! Optional left edges of sale-quantity plateaux on the unchanged buy grid.
//! A hint only: the native curve still validates each point and prices both legs.
use super::size::{Curve, Point};
use super::size_memo::Memo;
use rust_decimal::{Decimal, prelude::ToPrimitive};

const MAX_EXTRA: u32 = 24;

#[derive(Clone, Copy, Debug)]
pub struct Arrival {
    fixed: Decimal,
    tax_pct: Decimal,
}

impl Arrival {
    pub fn new(fixed: Decimal, tax_pct: Decimal) -> Option<Self> {
        (fixed >= Decimal::ZERO && tax_pct > Decimal::ZERO && tax_pct < Decimal::ONE_HUNDRED)
            .then_some(Self { fixed, tax_pct })
    }

    /// Use the same order of operations as transfer_cost::cost and costs_exit::calculate.
    fn amount(self, qty: Decimal) -> Option<Decimal> {
        let sent = qty.checked_sub(self.fixed)?;
        sent.checked_sub(
            sent.checked_mul(self.tax_pct)?
                .checked_div(Decimal::ONE_HUNDRED)?,
        )
    }

    fn edges(self, step: Decimal, lo: u64, hi: u64, anchor: u64) -> Option<Vec<u64>> {
        let loss = self.tax_pct.checked_div(Decimal::ONE_HUNDRED)?;
        let intercept = self
            .fixed
            .checked_mul(Decimal::ONE.checked_sub(loss)?)?
            .checked_div(step)?;
        let pos = |n: u64| Decimal::from(n).checked_mul(loss)?.checked_add(intercept);
        let first = pos(lo)?.ceil();
        let last = pos(hi.checked_add(1)?)?.ceil().checked_sub(Decimal::ONE)?;
        if last < first {
            return Some(Vec::new());
        }
        let center = pos(anchor)?.round().clamp(first, last);
        let mut edges = Vec::new();
        for radius in 0..=2 * MAX_EXTRA {
            let delta = Decimal::from(radius);
            for edge in [center.checked_sub(delta)?, center.checked_add(delta)?] {
                if edge < first || edge > last {
                    continue;
                }
                let Some(n) = edge
                    .checked_sub(intercept)?
                    .checked_div(loss)?
                    .floor()
                    .to_u64()
                else {
                    continue;
                };
                if n < lo || n >= hi || edges.contains(&n) {
                    continue;
                }
                let q = Decimal::from(n).checked_mul(step)?;
                let a = self.amount(q)?.checked_div(step)?.floor();
                let b = self
                    .amount(q.checked_add(step)?)?
                    .checked_div(step)?
                    .floor();
                if a == b {
                    edges.push(n);
                }
            }
            if center - delta <= first && center + delta >= last {
                break;
            }
        }
        Some(edges)
    }
}

pub(super) async fn probe<C: Curve>(
    m: &mut Memo<'_, C>,
    arrival: Arrival,
    (lo, hi): (u64, u64),
    best: &mut Point<C::Data>,
) {
    if m.sell_grid.is_some()
        || !m
            .map
            .values()
            .flatten()
            .all(|p| arrival.amount(p.qty) == Some(p.arrive))
    {
        return;
    }
    let Some(edges) = m
        .units(best.qty)
        .and_then(|anchor| arrival.edges(m.step, lo, hi, anchor))
    else {
        return;
    };
    let mut remaining = MAX_EXTRA.min(m.max.saturating_sub(m.done));
    let gate = m.gate();
    for n in edges {
        if remaining == 0
            || m.capped()
            || m.deadline.is_some_and(|d| std::time::Instant::now() >= d)
        {
            break;
        }
        if m.map.contains_key(&n) {
            continue;
        }
        remaining -= 1;
        let point = match m.at(n).await {
            Ok(p) => p,
            Err(super::size::Fail::Stop(_)) => break,
            Err(_) => continue,
        };
        if arrival.amount(point.qty) != Some(point.arrive) {
            break;
        }
        if gate.passes::<C>(&point)
            && (!gate.passes::<C>(best)
                || point.profit > best.profit
                || point.profit == best.profit
                    && (point.invested, point.qty) < (best.invested, best.qty))
        {
            *best = point;
        }
    }
}
