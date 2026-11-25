//! Bounded hints learned from prices the candidate search already evaluated. A hint
//! provides only a quantity; both final results come from the ordinary execution quote.
use super::{
    costs::Calc,
    dirctx::DirCtx,
    engine::{Core, lk, rd},
    size::{Curve, Found},
    size_memo::Memo,
};
use crate::core::types::{CostKind, Direction, PoolKind};
use chrono::{DateTime, Utc};
use futures_util::FutureExt;
use rust_decimal::{Decimal, MathematicalOps};
use std::{collections::VecDeque, sync::Arc};

const MAX_DIRECTIONS: usize = 32;
const MAX_POINTS: usize = 300;

#[derive(Default)]
pub(super) struct Cache(VecDeque<(Direction, Arc<Samples>)>);

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Stamp {
    book: DateTime<Utc>,
    pools: u64,
}
#[derive(Clone, Copy)]
struct Sample {
    qty: Decimal,
    bought_at: Decimal,
    sold_at: Decimal,
    invested: Decimal,
    spend: Decimal,
    proceeds: Decimal,
    before_reserve: Decimal,
}
struct Samples {
    stamp: Stamp,
    anchor: Decimal,
    keep: Decimal,
    fee: Option<Decimal>,
    risk_only: bool,
    points: Vec<Sample>,
}

pub(super) fn enabled(core: &Core, cx: &DirCtx) -> bool {
    cx.units_b_per_a.is_identity()
        && cx.pair_a.is_some() != cx.pair_b.is_some()
        && core
            .inner
            .deps
            .settings
            .flag(super::precheck_fast::FAST, Default::default())
            .unwrap_or(false)
}
pub(super) fn stamp(core: &Core, cx: &DirCtx) -> Option<Stamp> {
    let token = cx.dir.asset.contract?;
    let (cex, ticker, dex) = if cx.pair_a.is_some() {
        (&cx.dir.buy_on, &cx.a.ticker, &cx.dir.sell_on)
    } else {
        (&cx.dir.sell_on, &cx.b.ticker, &cx.dir.buy_on)
    };
    let book = rd(&core.inner.market).book(cex, ticker)?.received_at;
    let pools = lk(&core.inner.quote_cache).generation(dex, token);
    Some(Stamp { book, pools })
}

pub(super) fn remember<C: Curve<Data = Calc>>(
    core: &Core,
    cx: &DirCtx,
    mark: Stamp,
    memo: &Memo<'_, C>,
    found: &Found<Calc>,
) {
    let Some(keep) = cx.keep else { return };
    if found.early || stamp(core, cx) != Some(mark) || memo.map.len() > MAX_POINTS {
        return;
    }
    let risk_only = cx.pair_a.is_some();
    let route = |c: &Calc| {
        if risk_only {
            c.sell_quote.route.as_ref().cloned()
        } else {
            c.buy_quote.route.as_ref().cloned()
        }
    };
    let Some(base) = route(&found.best.data) else {
        return;
    };
    let [leg] = base.legs.as_slice() else { return };
    let eligible = if risk_only {
        leg.hops.len() == 1 && leg.hops[0].kind == PoolKind::V2
    } else {
        leg.hops.len() == 2 && leg.hops.iter().all(|h| h.kind == PoolKind::V3)
    };
    if !base.executable || !eligible {
        return;
    }
    let fee = if risk_only {
        None
    } else {
        let c = &found.best.data;
        if c.proceeds <= Decimal::ZERO || c.sell_qty != c.qty {
            return;
        }
        let sum = c
            .costs
            .iter()
            .filter(|x| x.kind == CostKind::TradeFee && x.stage == "B" && !x.in_price && !x.in_qty)
            .map(|x| x.usdt)
            .sum::<Option<Decimal>>();
        let Some(rate) = sum.and_then(|n| n.checked_div(c.proceeds)) else {
            return;
        };
        Some(rate)
    };
    let points: Vec<_> = memo
        .map
        .values()
        .flatten()
        .filter_map(|p| {
            let c = &p.data;
            let r = if risk_only {
                c.sell_quote.route.as_ref()
            } else {
                c.buy_quote.route.as_ref()
            }?;
            let [l] = r.legs.as_slice() else { return None };
            if r.block != base.block
                || !r.executable
                || l.hops != leg.hops
                || (!risk_only && c.sell_qty != c.qty)
            {
                return None;
            }
            let reserve = c
                .costs
                .iter()
                .find(|x| x.kind == CostKind::PriceMoveReserve)?
                .usdt?;
            Some(Sample {
                qty: p.qty,
                bought_at: c.buy_avg,
                sold_at: c.sell_avg,
                invested: c.invested,
                spend: c.spend_max,
                proceeds: c.proceeds,
                before_reserve: c.profit_usd + reserve,
            })
        })
        .collect();
    if points.len() < 2 {
        return;
    }
    let history = Arc::new(Samples {
        stamp: mark,
        anchor: found.best.qty,
        keep,
        fee,
        risk_only,
        points,
    });
    let mut cache = lk(&core.inner.precheck_hints);
    cache.0.retain(|(d, _)| d != &cx.dir);
    if cache.0.len() >= MAX_DIRECTIONS {
        cache.0.pop_front();
    }
    cache.0.push_back((cx.dir.clone(), history));
}

/// Short-lived candidate curves never carry authority to submit a trade.
pub(super) fn alternative(
    core: &Core,
    cx: &DirCtx,
    qty: Decimal,
    budget: Decimal,
    step: Decimal,
    floor: (Decimal, Decimal),
) -> Option<Decimal> {
    let samples = lk(&core.inner.precheck_hints)
        .0
        .iter()
        .find(|(d, s)| d == &cx.dir && s.anchor == qty)
        .map(|(_, s)| s.clone())?;
    let mark = stamp(core, cx)?;
    let keep = cx.keep?;
    if samples.stamp.pools != mark.pools || !cx.units_b_per_a.is_identity() {
        return None;
    }
    if samples.risk_only {
        if samples.stamp.book != mark.book || samples.keep == keep {
            return None;
        }
        let mut best = None;
        for p in &samples.points {
            let risk =
                super::reserve::amount(keep, p.proceeds, cx.closure_share, p.bought_at, p.sold_at);
            let pnl = p.before_reserve - risk;
            if p.invested > Decimal::ZERO
                && p.spend <= budget
                && pnl >= floor.0
                && pnl / p.invested * Decimal::ONE_HUNDRED >= floor.1
            {
                let rank = (pnl, std::cmp::Reverse((p.spend, p.qty)));
                if best.is_none_or(|b| rank > b) {
                    best = Some(rank);
                }
            }
        }
        return best
            .map(|(_, std::cmp::Reverse((_, q)))| q)
            .filter(|q| *q != qty);
    }
    let top = rd(&core.inner.market)
        .book(&cx.dir.sell_on, &cx.b.ticker)?
        .bids
        .first()?
        .clone();
    let marginal = top.price.checked_mul(keep.checked_sub(samples.fee?)?)?;
    // Within one V3 liquidity segment the average price for exact output is
    // 1/(a-b*q). Price only inside the interval supported by the two probes and
    // the actual sell level. Crossing a tick may distort the hint, never the final quote.
    let mut candidate = None;
    for pair in samples.points.windows(2) {
        let (left, right) = (pair[0], pair[1]);
        if left.spend > budget || left.qty > top.qty * cx.h || right.qty < step {
            continue;
        }
        let Some(q) = segment(
            left.qty,
            left.bought_at,
            right.qty,
            right.bought_at,
            marginal,
            step,
        ) else {
            continue;
        };
        if q > top.qty * cx.h || q == qty || q <= Decimal::ZERO || q * left.bought_at > budget {
            continue;
        }
        let dist = (q - qty).abs();
        if candidate.is_none_or(|(old, _)| dist < old) {
            candidate = Some((dist, q));
        }
    }
    candidate.map(|(_, q)| q)
}

/// Stationary output of two constant-liquidity V3 hops within the observed interval.
/// The hint may be rejected by the complete native quote (ticks, second book leg, fees).
pub fn segment(
    q0: Decimal,
    p0: Decimal,
    q1: Decimal,
    p1: Decimal,
    marginal: Decimal,
    step: Decimal,
) -> Option<Decimal> {
    if q1 <= q0
        || p0 <= Decimal::ZERO
        || p1 <= p0
        || marginal <= Decimal::ZERO
        || step <= Decimal::ZERO
    {
        return None;
    }
    let r = Decimal::ONE.checked_div(p0)?;
    let b = r
        .checked_sub(Decimal::ONE.checked_div(p1)?)?
        .checked_div(q1.checked_sub(q0)?)?;
    if b <= Decimal::ZERO {
        return None;
    }
    let a = r.checked_add(b.checked_mul(q0)?)?;
    let q = a
        .checked_sub(a.checked_div(marginal)?.sqrt()?)?
        .checked_div(b)?;
    let q = q.checked_sub(q.checked_rem(step)?)?;
    (q >= q0 && q <= q1).then_some(q)
}

/// Poll the incumbent first and accept an alternative only if it is ready by its
/// completion. A pending optional future is dropped, including on caller cancellation.
pub async fn compare<T, E, A, B, F>(first: A, second: B, better: F) -> (Result<T, E>, bool)
where
    A: std::future::Future<Output = Result<T, E>>,
    B: std::future::Future<Output = Result<T, E>>,
    F: FnOnce(&T, &T) -> bool,
{
    tokio::pin!(first, second);
    if let Some(ready) = first.as_mut().now_or_never() {
        return (ready, false);
    }
    let (original, alternative) = tokio::select! {
        biased;
        alternative=&mut second=>(first.await,Some(alternative)),
        original=&mut first=>(original,None),
    };
    if let Some(Ok(alt)) = alternative
        && original.as_ref().ok().is_none_or(|old| better(&alt, old))
    {
        (Ok(alt), true)
    } else {
        (original, false)
    }
}
