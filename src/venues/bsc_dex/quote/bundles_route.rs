//! Повторные обращения к BUN/WETH считают состояния по порядку вызовов V5, включая изменение часового оракула.
use super::super::{ctx::Ctx, math::bundles_sequence::Sequence, num};
use super::{
    path::Path,
    route_calc::{self, Leg, keep},
};
use crate::{
    core::types::{PoolRef, PoolState, Side},
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy_primitives::U256;
use rust_decimal::Decimal;
use std::collections::HashSet;

pub(super) fn affected(l: &Leg, common: &HashSet<PoolRef>) -> bool {
    common.contains(&l.entry.liquidity_site())
        || l.entry.second_liquidity_site().is_some_and(|s|common.contains(&s))
        || matches!(&l.entry.pool.state,PoolState::Bundles(b) if common.contains(&PoolRef::Contract(b.fee_pair.pool)))
}
pub(super) fn shares(a: &Leg, b: &Leg) -> bool {
    matches!((&a.entry.pool.state,&b.entry.pool.state),(PoolState::Bundles(x),PoolState::Bundles(y)) if x.fee_pair.pool==y.fee_pair.pool)
}
pub(super) fn coupled(legs: &[Leg]) -> bool {
    legs.iter().enumerate().any(|(i, l)| {
        matches!(&l.entry.pool.state, PoolState::Bundles(_))
            && legs[..i].iter().any(|k| shares(l, k))
    })
}
pub(super) struct Forward {
    legs: Vec<Leg>,
    steps: Vec<(Decimal, super::super::math::swap_cl::Swapped)>,
    output: Decimal,
}
impl Forward {
    fn finish(self, ctx: &Ctx, coins: Decimal, usdt: Decimal) -> GatewayResult<Path> {
        let (pcts, swaps) = self.steps.into_iter().unzip::<_, _, Vec<_>, Vec<_>>();
        route_calc::build(ctx, &self.legs, swaps, &pcts, (coins, usdt))
    }
}
fn forward(ctx: &Ctx, base: &[Leg], mut amount: U256) -> GatewayResult<Forward> {
    let mut series = Sequence::default();
    let mut steps = Vec::with_capacity(base.len());
    let mut legs = base.to_vec();
    let mut output = Decimal::ZERO;
    let n = legs.len();
    for (i, l) in legs.iter_mut().enumerate() {
        if let PoolState::Bundles(b) = &l.entry.pool.state {
            l.entry.pool.state = PoolState::Bundles(Box::new(series.current(b)));
        }
        let (pct, s) = route_calc::sell_step(ctx, l, amount)?;
        if !s.complete {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "Неполная выплата на последовательном маршруте",
            ));
        }
        if let PoolState::Bundles(b) = &l.entry.pool.state {
            series.advance(b, l.token_in, amount).ok_or_else(|| {
                ctx.err(
                    ErrorCode::NoRoute,
                    "Bundles Swap: комиссия общей пары не подтверждена",
                )
            })?;
        }
        output = num::to_decimal(s.amount_out, l.dec_out).ok_or_else(|| route_calc::bad(ctx))?
            * keep(pct);
        steps.push((pct, s));
        if i + 1 < n {
            amount = num::to_units(output, l.dec_out).ok_or_else(|| route_calc::bad(ctx))?
        }
    }
    Ok(Forward {
        legs,
        steps,
        output,
    })
}
pub(super) fn eval(
    ctx: &Ctx,
    legs: &[Leg],
    side: Side,
    qty: Decimal,
    tax: Decimal,
) -> GatewayResult<Path> {
    let first = legs.first().ok_or_else(|| route_calc::bad(ctx))?;
    if side == Side::Sell {
        let n = num::to_units(qty * tax, first.dec_in).ok_or_else(|| route_calc::bad(ctx))?;
        let f = forward(ctx, legs, n)?;
        let usdt = f.output;
        return f.finish(ctx, qty, usdt);
    }
    let estimate = route_calc::buy(ctx, legs, qty, tax, None)?;
    let initial = estimate.hops[0].swapped.amount_in;
    let zero = U256::ZERO;
    let enough = |v: &Forward| v.output * tax >= qty;
    let mut low = zero;
    let mut high = initial;
    let mut step = U256::ONE;
    let mut proven = forward(ctx, legs, high)?;
    if enough(&proven) {
        // Ищем ближайшую нижнюю границу от оценки, не обходим весь диапазон USDT.
        while high > U256::ONE {
            let next = high.saturating_sub(step).max(U256::ONE);
            match forward(ctx, legs, next) {
                Ok(v) if enough(&v) => {
                    high = next;
                    proven = v;
                }
                _ => {
                    low = next;
                    break;
                }
            }
            step = step.saturating_mul(U256::from(2));
        }
    } else {
        loop {
            low = high;
            high = high.checked_add(step).ok_or_else(|| route_calc::bad(ctx))?;
            match forward(ctx, legs, high) {
                Ok(v) if enough(&v) => {
                    proven = v;
                    break;
                }
                Ok(_) => {
                    step = step.saturating_mul(U256::from(2));
                }
                Err(e) => {
                    // Перескочили верхнюю границу исполнимости; проверяем промежуток перед ней.
                    let mut end = high;
                    loop {
                        if end - low <= U256::ONE {
                            return Err(e);
                        }
                        let q = low + ((end - low) >> 1);
                        match forward(ctx, legs, q) {
                            Ok(v) if enough(&v) => {
                                high = q;
                                proven = v;
                                break;
                            }
                            Ok(_) => low = q,
                            Err(_) => end = q,
                        }
                    }
                    break;
                }
            }
        }
    }
    while high - low > U256::ONE {
        let q = low + ((high - low) >> 1);
        match forward(ctx, legs, q) {
            Ok(v) if enough(&v) => {
                high = q;
                proven = v;
            }
            _ => low = q,
        }
    }
    let usdt = num::to_decimal(high, first.dec_in).ok_or_else(|| route_calc::bad(ctx))?;
    proven.finish(ctx, qty, usdt)
}
