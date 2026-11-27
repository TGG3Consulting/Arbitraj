//! Одиночный ордер конкурирует с текущим маршрутом по чистой стоимости с газом.
use super::{model::Offer, profile::Profile, simulate::Payment, *};
use crate::{
    core::types::{Asset, Cost, CostKind, DexRoute, Quote, RouteMode, Side},
    gateway::quote_stage,
    venues::bsc_dex::{exec, gas_ticker, hashflow, num, quote::direct, send},
};
use alloy_primitives::keccak256;
use rust_decimal::Decimal;
use std::{sync::Arc, time::Duration};
pub(in crate::venues::bsc_dex) struct Ticket {
    pub(super) offer: Arc<Offer>,
    pub(super) owner: Address,
    pub(super) quantity: U256,
    pub(super) spent: U256,
    pub(super) received: U256,
    pub(super) qty: Decimal,
    pub(super) side: Side,
    pub(super) decs: (u8, u8),
    pub(super) generation: u64,
    pub(in crate::venues::bsc_dex) fallback_fee: Option<Decimal>,
}
impl Ticket {
    pub(super) fn id(&self) -> B256 {
        let mut s = b"Rubicon/Gladius/ERC20".to_vec();
        s.extend_from_slice(self.offer.api.order_hash.as_slice());
        s.extend_from_slice(self.owner.as_slice());
        s.extend(self.quantity.to_be_bytes::<32>());
        s.extend(self.spent.to_be_bytes::<32>());
        keccak256(s)
    }
}
fn pair(p: &Parts, a: &Asset, s: Side) -> Option<(Address, Address)> {
    let coin = a.contract?;
    let quote = p.tokens.usdt(&p.ctx).ok()?;
    (!exec::native::is_native(&p.ctx, a) && coin != quote).then_some(if s == Side::Buy {
        (quote, coin)
    } else {
        (coin, quote)
    })
}
pub(in crate::venues::bsc_dex) fn available(p: &Parts, a: &Asset, s: Side) -> bool {
    enabled(&p.ctx)
        && pair(p, a, s)
            .zip(p.gladius.rows(p))
            .is_some_and(|(pair, rows)| {
                rows.iter()
                    .any(|r| r.pair() == pair && timely(&p.ctx, r.expiry))
            })
}
pub(in crate::venues::bsc_dex) fn breaks(
    p: &Parts,
    a: &Asset,
    s: Side,
    max: Decimal,
) -> Vec<Decimal> {
    let Some(((i, o), rows)) = enabled(&p.ctx)
        .then(|| pair(p, a, s).zip(p.gladius.rows(p)))
        .flatten()
    else {
        return vec![];
    };
    let Some(d) = a.contract.and_then(|a| p.tokens.decimals_known(a)) else {
        return vec![];
    };
    rows.iter()
        .filter(|v| v.pair() == (i, o) && timely(&p.ctx, v.expiry))
        .flat_map(|v| {
            let q = v.order.threshold.max(U256::ONE);
            let limits = if s == Side::Buy {
                [Some(q), Some(v.quantity)]
            } else {
                [
                    q.checked_mul(v.debit).map(|n| n.div_ceil(v.quantity)),
                    Some(v.debit),
                ]
            };
            limits
                .into_iter()
                .flatten()
                .filter_map(|n| num::to_decimal(n, d))
                .filter(|n| *n <= max && *n > Decimal::ZERO)
        })
        .collect()
}
pub(in crate::venues::bsc_dex) async fn consider(
    p: &Parts,
    a: &Asset,
    s: Side,
    qty: Decimal,
    old: GatewayResult<Quote>,
) -> GatewayResult<Quote> {
    if !available(p, a, s)
        || qty <= Decimal::ZERO
        || old.as_ref().is_err_and(|e| e.code != ErrorCode::NoRoute)
    {
        return old;
    }
    let Some((pair, rows)) = pair(p, a, s).zip(p.gladius.rows(p)) else {
        return old;
    };
    let ctx = &p.ctx;
    let generation = ctx.settings.generation();
    let Some(profile) = Profile::read(ctx) else {
        return old;
    };
    let mut best = None;
    let mut cutoff = if let Ok(q) = &old
        && q.fillable_qty == qty
        && (!quote_stage::is_final() || q.route.as_ref().is_none_or(|r| r.executable))
    {
        let Some(total) = hashflow::routing::total(p, q).await else {
            return old;
        };
        Some(total)
    } else {
        None
    };
    let Ok(ms) = ctx.int("gladius.quote_timeout_ms") else {
        return old;
    };
    let end = tokio::time::Instant::now() + Duration::from_millis(ms);
    use futures_util::{StreamExt, stream};
    let candidates = stream::iter(
        rows.iter()
            .filter(|v| v.pair() == pair && timely(ctx, v.expiry))
            .cloned(),
    )
    .map(|offer| candidate(p, a, s, qty, profile, offer))
    .buffer_unordered(4);
    tokio::pin!(candidates);
    while let Ok(Some(row)) = tokio::time::timeout_at(end, candidates.next()).await {
        let Some((q, t, total)) = row else {
            continue;
        };
        if cutoff.is_none_or(|c| if s == Side::Buy { total < c } else { total > c }) {
            cutoff = Some(total);
            best = Some((q, t));
        }
    }
    if ctx.settings.generation() != generation {
        return old;
    }
    if let Some((q, t)) = best {
        if let Some(t) = t {
            p.gladius.save(t);
        }
        return Ok(q);
    }
    old
}
async fn candidate(
    p: &Parts,
    a: &Asset,
    s: Side,
    qty: Decimal,
    profile: Profile,
    offer: Arc<Offer>,
) -> Option<(Quote, Option<Ticket>, Decimal)> {
    let ctx = &p.ctx;
    let generation = ctx.settings.generation();
    p.lag.check(ctx).ok()?;
    let mut pricing = p.quoting();
    direct::taxes(&pricing, a).ok()?;
    let (i, o) = offer.pair();
    let buy = s == Side::Buy;
    let age = Duration::from_millis(ctx.int("gladius.max_age_ms").ok()?);
    let funds = p
        .gladius
        .funds
        .get(p, profile.chain, i, profile.module.address, age)?;
    let di = funds.decimals;
    let dout = p.tokens.decimals_known(o)?;
    let decs = if buy { (dout, di) } else { (di, dout) };
    let wanted = direct::ceil_units(qty, decs.0)?;
    if num::to_decimal(wanted, decs.0)? != qty
        || funds.who == offer.order.info.swapper
        || offer.order.outputs[0].recipient == funds.who
    {
        return None;
    }
    let gas_cap = ctx.dec("gladius.gas_budget_native").ok()?;
    if gas_cap <= Decimal::ZERO
        || funds.native < num::to_units(gas_cap, 18)?
        || send::tip(&send::select(ctx, &p.stats).ok()?).is_some()
    {
        return None;
    }
    let mut quantity = if buy {
        wanted
    } else {
        wanted
            .checked_mul(offer.quantity)?
            .checked_div(offer.debit)?
    };
    if quantity.is_zero() || quantity > offer.quantity || quantity < offer.order.threshold {
        return None;
    }
    let predicted = quantity.checked_mul(offer.debit)?.div_ceil(offer.quantity);
    if predicted > funds.spend || (!buy && wanted > funds.spend) {
        return None;
    }
    let mut cash = num::to_decimal(if buy { predicted } else { quantity }, decs.1)?;
    let mut gas = gas_cap;
    let mut units = 0;
    let mut block = p.heads.fresh(ctx).ok()?.number;
    let mut ticket = None;
    if quote_stage::is_final() {
        let h = white_state::head(ctx, "latest").await.ok()?;
        profile.verify(ctx, h).await.ok()?;
        block = h.number.to();
        let mut debit = offer.quote(ctx, &profile, quantity, h).await.ok()?;
        if !buy && debit != wanted {
            let adjusted = quantity.checked_mul(wanted)?.div_ceil(debit);
            let mut found = None;
            for next in [
                Some(adjusted),
                adjusted.checked_sub(U256::ONE),
                adjusted.checked_add(U256::ONE),
            ]
            .into_iter()
            .flatten()
            {
                if next == quantity
                    || next.is_zero()
                    || next > offer.quantity
                    || next < offer.order.threshold
                {
                    continue;
                }
                if offer.quote(ctx, &profile, next, h).await.ok() == Some(wanted) {
                    found = Some(next);
                    break;
                }
            }
            quantity = found?;
            debit = wanted;
        }
        if debit > funds.spend {
            return None;
        }
        let payment = Payment::new(
            profile,
            funds.who,
            &offer,
            quantity,
            if buy { U256::ZERO } else { wanted },
            debit,
            quantity,
        );
        let (spent, received, gas_used) = payment.measure(ctx, h, offer.expiry).await.ok()?;
        cash = num::to_decimal(if buy { spent } else { received }, decs.1)?;
        pricing.gas = pricing.gas.for_quote(ctx, &p.heads).await.ok()?;
        let bill =
            exec::cycle_gas::priced(&pricing, funds.who, (profile.module.address, payment.data))
                .await
                .ok()?;
        let wei = exec::cycle_gas::simulated(&pricing, bill, gas_used, 0, 0)
            .ok()?
            .wei();
        gas = num::to_decimal(wei, 18)?;
        if gas > gas_cap || !timely(ctx, offer.expiry) {
            return None;
        }
        units = gas_used;
        ticket = Some(Ticket {
            offer,
            owner: funds.who,
            quantity,
            spent,
            received,
            qty,
            side: s,
            decs,
            generation,
            fallback_fee: None,
        });
    }
    let q = Quote {
        venue: ctx.venue(),
        asset: a.clone(),
        side: s,
        qty,
        fillable_qty: qty,
        avg_price: cash.checked_div(qty)?,
        costs: vec![Cost::new(
            CostKind::Gas,
            gas,
            gas_ticker::read(ctx).ok()?,
            false,
        )],
        route: Some(DexRoute {
            mode: ticket
                .as_ref()
                .map_or(RouteMode::RfqIndicative, |t| RouteMode::Rfq(t.id())),
            legs: vec![],
            block,
            gas_units: units,
            executable: ticket.is_some(),
            exec_note: Some(LABEL.into()),
        }),
        signal_route: None,
        at: Utc::now(),
        pick: Some(LABEL.into()),
    };
    let total = hashflow::routing::total(p, &q).await?;
    if let Some(t) = &mut ticket {
        t.fallback_fee = Some((total - cash).abs());
    }
    Some((q, ticket, total))
}
