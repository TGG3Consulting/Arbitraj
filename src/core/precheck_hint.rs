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
impl Sample {
    fn from_calc(c: &Calc) -> Option<Self> {
        let reserve = c
            .costs
            .iter()
            .find(|x| x.kind == CostKind::PriceMoveReserve)?
            .usdt?;
        Some(Self {
            qty: c.qty,
            bought_at: c.buy_avg,
            sold_at: c.sell_avg,
            invested: c.invested,
            spend: c.spend_max,
            proceeds: c.proceeds,
            before_reserve: c.profit_usd + reserve,
        })
    }
}

struct Samples {
    stamp: Stamp,
    anchor: Decimal,
    keep: Decimal,
    fee: Option<Decimal>,
    risk_only: bool,
    budget: Decimal,
    step: Decimal,
    floor: Option<(Decimal, Decimal)>,
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
    let c = &found.best.data;
    let Some(base) = (if risk_only {
        c.sell_quote.route.as_ref()
    } else {
        c.buy_quote.route.as_ref()
    }) else {
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
            Sample::from_calc(c)
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
        budget: memo.budget,
        step: memo.step,
        floor: memo.gate().min_usd.zip(memo.gate().min_pct),
        points,
    });
    let mut cache = lk(&core.inner.precheck_hints);
    cache.0.retain(|(d, _)| d != &cx.dir);
    if cache.0.len() >= MAX_DIRECTIONS {
        cache.0.pop_front();
    }
    cache.0.push_back((cx.dir.clone(), history));
}

fn saved(core: &Core, d: &Direction, qty: Decimal) -> Option<Arc<Samples>> {
    lk(&core.inner.precheck_hints)
        .0
        .iter()
        .find(|(key, entry)| key == d && entry.anchor == qty)
        .map(|(_, s)| s.clone())
}

#[path = "precheck_hint_warm.rs"]
mod warm;
pub(super) use warm::{during, ready};

/// Short-lived candidate curves never carry authority to submit a trade.
pub(super) async fn alternative(
    core: &Core,
    cx: &DirCtx,
    qty: Decimal,
    budget: Decimal,
    step: Decimal,
    floor: (Decimal, Decimal),
    slot: tokio::sync::OwnedSemaphorePermit,
) -> Option<(Decimal, Stamp, tokio::sync::OwnedSemaphorePermit)> {
    let samples = saved(core, &cx.dir, qty)?;
    let mark = stamp(core, cx)?;
    let keep = cx.keep?;
    if samples.stamp.pools != mark.pools || !cx.units_b_per_a.is_identity() {
        return None;
    }
    if samples.risk_only && (samples.stamp.book != mark.book || samples.keep == keep) {
        return None;
    }
    let bids = if samples.risk_only {
        Vec::new()
    } else {
        rd(&core.inner.market)
            .book(&cx.dir.sell_on, &cx.b.ticker)?
            .bids
            .iter()
            .take(64)
            .cloned()
            .collect()
    };
    let model = Model {
        keep,
        share: cx.closure_share,
        h: cx.h,
        budget,
        step,
        floor,
    };
    use crate::gateway::cpu_budget;
    let cpu = cpu_budget::try_optional(cpu_budget::limits(&core.inner.deps.settings))?;
    tokio::task::spawn_blocking(move || {
        let _cpu = cpu;
        let choice = if samples.risk_only {
            risk_best(&samples.points, &model)
        } else {
            v3::best(&samples.points, &bids, &model, samples.fee?)
        };
        choice.filter(|q| *q != qty).map(|q| (q, mark, slot))
    })
    .await
    .ok()?
}

#[path = "precheck_hint_v3.rs"]
mod v3;

struct Model {
    keep: Decimal,
    share: Option<Decimal>,
    h: Decimal,
    budget: Decimal,
    step: Decimal,
    floor: (Decimal, Decimal),
}

fn risk_best(points: &[Sample], model: &Model) -> Option<Decimal> {
    let Model {
        keep,
        share,
        budget,
        floor,
        ..
    } = *model;
    points
        .iter()
        .filter_map(|p| {
            let risk = super::reserve::amount(keep, p.proceeds, share, p.bought_at, p.sold_at);
            let pnl = p.before_reserve - risk;
            (p.invested > Decimal::ZERO
                && p.spend <= budget
                && pnl >= floor.0
                && pnl / p.invested * Decimal::ONE_HUNDRED >= floor.1)
                .then_some((pnl, std::cmp::Reverse((p.spend, p.qty))))
        })
        .max()
        .map(|(_, std::cmp::Reverse((_, q)))| q)
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
    if marginal < a.checked_mul(p0)?.checked_mul(p0)?
        || marginal > a.checked_mul(p1)?.checked_mul(p1)?
    {
        return None;
    }
    let q = a
        .checked_sub(a.checked_div(marginal)?.sqrt()?)?
        .checked_div(b)?;
    (q >= q0 && q <= q1).then_some(q)
}

/// Poll the incumbent first and accept an alternative only if it is ready by its
/// completion. A pending optional future is dropped, including on caller cancellation.
pub async fn compare<T, E, A, B, F>(
    first: A,
    second: B,
    prepared: bool,
    better: F,
) -> (Result<T, E>, bool)
where
    A: std::future::Future<Output = Result<T, E>>,
    B: std::future::Future<Output = Result<T, E>>,
    F: FnOnce(&T, &T) -> bool,
{
    tokio::pin!(first, second);
    let (original, alternative) = if let Some(ready) = first.as_mut().now_or_never() {
        let other = if prepared && ready.is_ok() {
            second.as_mut().now_or_never()
        } else {
            None
        };
        (ready, other)
    } else {
        tokio::select! {
            biased;
            alternative=&mut second=>(first.await,Some(alternative)),
            original=&mut first=>(original,None),
        }
    };
    if let (Ok(incumbent), Some(Ok(alt))) = (&original, alternative)
        && better(&alt, incumbent)
    {
        (Ok(alt), true)
    } else {
        (original, false)
    }
}

#[cfg(test)]
mod tests {
    use super::compare;
    use std::future::{pending, ready};

    async fn later<T>(value: T) -> Result<T, ()> {
        tokio::task::yield_now().await;
        Ok(value)
    }
    fn larger(a: &i64, b: &i64) -> bool {
        a > b
    }
    #[tokio::test]
    async fn slow_optional_quote_and_late_market_do_not_delay_or_replace_the_incumbent() {
        let never = pending::<Result<i64, ()>>();
        assert_eq!(
            compare(later(4), never, false, larger).await,
            (Ok(4), false)
        );
        assert_eq!(
            compare(ready(Ok::<_, ()>(4)), ready(Ok(6)), true, larger).await,
            (Ok(6), true)
        );
        assert_eq!(
            compare(ready(Ok::<_, ()>(4)), pending(), true, larger).await,
            (Ok(4), false)
        );
        assert_eq!(
            compare(
                ready(Err::<i64, ()>(())),
                async { panic!("a rejected ready incumbent started optional work") },
                true,
                larger
            )
            .await,
            (Err(()), false)
        );
        let refused = async {
            tokio::task::yield_now().await;
            Err::<i64, ()>(())
        };
        assert_eq!(
            compare(refused, ready(Ok(6)), true, larger).await,
            (Err(()), false)
        );
        let untouched = async {
            panic!("an immediately ready incumbent started optional work");
        };
        assert_eq!(
            compare(ready(Ok::<_, ()>(3)), untouched, false, larger).await,
            (Ok(3), false)
        );
        assert_eq!(
            compare(later(5), ready(Ok(6)), false, larger).await,
            (Ok(6), true)
        );
        assert_eq!(
            compare(later(5), ready(Ok(6)), false, |_, _| false).await,
            (Ok(5), false)
        );
        assert_eq!(
            compare(later(5), ready(Err(())), false, larger).await,
            (Ok(5), false)
        );
    }
}
