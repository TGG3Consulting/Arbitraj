//! Suggest one size on a sampled two-hop V3 buy / CEX sell. The CEX book is
//! consumed cumulatively and the hint never evaluates outside sampled buy sizes.
use super::{DirCtx, Sample, segment};
use crate::core::types::PriceLevel;
use rust_decimal::Decimal as D;

#[derive(Clone, Copy)]
struct Band {
    lo: D,
    hi: D,
    price: D,
    before: D,
}

pub(super) fn best(
    points: &[Sample],
    bids: &[PriceLevel],
    cx: &DirCtx,
    fee: D,
    budget: D,
    step: D,
    floor: (D, D),
) -> Option<D> {
    let keep = cx.keep?;
    let largest = points.last()?.qty;
    if cx.h <= D::ZERO || cx.h > D::ONE || fee < D::ZERO || fee >= keep || step <= D::ZERO {
        return None;
    }
    let needed = largest.checked_div(cx.h)?;
    let mut bands = Vec::new();
    let (mut depth, mut proceeds) = (D::ZERO, D::ZERO);
    // The optional calculation is bounded even if a venue supplies thousands of levels.
    for b in bids.iter().take(64) {
        if b.qty <= D::ZERO || b.price <= D::ZERO {
            return None;
        }
        let end = depth.checked_add(b.qty)?;
        bands.push(Band {
            lo: depth,
            hi: end,
            price: b.price,
            before: proceeds,
        });
        depth = end;
        proceeds = proceeds.checked_add(b.price.checked_mul(b.qty)?)?;
        if depth >= needed {
            break;
        }
    }
    let max_sell = depth.checked_mul(cx.h)?;
    let mut best = None;
    let mut j = 0;
    for pair in points.windows(2) {
        let (l, r) = (pair[0], pair[1]);
        if l.qty > max_sell {
            break;
        }
        while j < bands.len() && bands[j].hi < l.qty {
            j += 1;
        }
        for &b in &bands[j..] {
            if b.lo > r.qty {
                break;
            }
            let lo = l.qty.max(b.lo);
            let hi = r.qty.min(b.hi).min(max_sell);
            if lo > hi {
                continue;
            }
            let stationary = segment(
                l.qty,
                l.bought_at,
                r.qty,
                r.bought_at,
                b.price * (keep - fee),
                step,
            );
            for q in [Some(lo), Some(hi), stationary].into_iter().flatten() {
                let q = q - q.checked_rem(step)?;
                if q <= D::ZERO || q < lo || q > hi {
                    continue;
                }
                let weight = (q - l.qty) / (r.qty - l.qty);
                let interpolate = |a: D, z: D| a + (z - a) * weight;
                // 1/average-price is affine within a V3 liquidity segment for exact output.
                // Even on a changed route this is only a numeric proposal for final_calc.
                let inv = interpolate(
                    D::ONE.checked_div(l.bought_at)?,
                    D::ONE.checked_div(r.bought_at)?,
                );
                let price = D::ONE.checked_div(inv)?;
                let investment = q.checked_mul(price)?;
                let spend = investment.checked_mul(interpolate(
                    l.spend.checked_div(l.invested)?,
                    r.spend.checked_div(r.invested)?,
                ))?;
                if spend > budget || investment <= D::ZERO {
                    continue;
                }
                let gross = b.before + (q - b.lo) * b.price;
                let fixed =
                    |p: Sample| p.proceeds - p.invested - p.before_reserve - p.proceeds * fee;
                let risk =
                    super::super::reserve::amount(keep, gross, cx.closure_share, price, gross / q);
                let pnl = gross - investment - gross * fee - interpolate(fixed(l), fixed(r)) - risk;
                if pnl < floor.0 || pnl / investment * D::ONE_HUNDRED < floor.1 {
                    continue;
                }
                let rank = (pnl, std::cmp::Reverse((spend, q)));
                if best.is_none_or(|b| rank > b) {
                    best = Some(rank);
                }
            }
        }
    }
    best.map(|(_, std::cmp::Reverse((_, q)))| q)
}
