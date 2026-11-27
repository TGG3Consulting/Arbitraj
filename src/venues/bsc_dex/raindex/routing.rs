//! Одиночный ордер конкурирует с текущим маршрутом по чистой стоимости с газом.
use super::{payment::Payment, *};
use crate::{
    core::types::{Asset, Cost, CostKind, DexRoute, Quote, RouteMode, Side},
    gateway::quote_stage,
    venues::bsc_dex::{exec, gas_ticker, hashflow, num, quote::direct, send},
};
use alloy_primitives::keccak256;
use chrono::Utc;
use rust_decimal::Decimal;
use std::{sync::Arc, time::Duration};
pub(in crate::venues::bsc_dex) struct Ticket {
    pub(super) row: Arc<Live>,
    pub(super) owner: Address,
    pub(super) action: Bytes,
    pub(super) expiry: u64,
    pub(super) middle: U256,
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
        let mut id = b"Raindex/order".to_vec();
        id.extend_from_slice(self.row.leg.target().as_slice());
        id.extend_from_slice(self.owner.as_slice());
        id.extend(self.expiry.to_be_bytes());
        id.extend_from_slice(&self.action);
        keccak256(id)
    }
}
/// В денежном круге у книги один журналируемый своп. Остальные источники RFQ остаются самостоятельными.
pub(super) fn mode(mixed: bool, id: Option<B256>) -> RouteMode {
    match (mixed, id) {
        (true, Some(id)) => RouteMode::Orderbook(id),
        (true, None) => RouteMode::OrderbookIndicative,
        (false, Some(id)) => RouteMode::Rfq(id),
        (false, None) => RouteMode::RfqIndicative,
    }
}
pub(in crate::venues::bsc_dex) fn mixed_ticket(route: &DexRoute) -> Option<B256> {
    if let RouteMode::Orderbook(id) = route.mode
        && route.executable
        && route.exec_note.as_deref() == Some(LABEL)
    {
        return Some(id);
    }
    None
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
            .zip(p.raindex.rows(&p.ctx))
            .is_some_and(|(pair, rows)| rows.iter().any(|r| r.leg.pair() == pair))
}
pub(in crate::venues::bsc_dex) fn breaks(
    p: &Parts,
    a: &Asset,
    s: Side,
    max: Decimal,
) -> Vec<Decimal> {
    let Some((pair, rows)) = enabled(&p.ctx)
        .then(|| pair(p, a, s).zip(p.raindex.rows(&p.ctx)))
        .flatten()
    else {
        return vec![];
    };
    rows.iter()
        .filter(|r| r.leg.pair() == pair)
        .filter_map(|r| {
            let di = r.leg.decimals(r.leg.input, p)?;
            let dout = r.leg.decimals(r.leg.output, p)?;
            let cap = r.cap.cap(dout)?;
            if s == Side::Buy {
                num::to_decimal(cap, dout)
            } else {
                num::to_decimal(r.ratio.paid(cap, di, dout)?, di)
            }
        })
        .filter(|v| *v > Decimal::ZERO && *v <= max)
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
    let Some((pair, rows)) = pair(p, a, s).zip(p.raindex.rows(&p.ctx)) else {
        return old;
    };
    let ctx = &p.ctx;
    let generation = ctx.settings.generation();
    let mut best = None;
    let mut cutoff = if let Ok(q) = &old
        && q.fillable_qty == qty
        && (!quote_stage::is_final() || q.route.as_ref().is_none_or(|r| r.executable))
        && (!quote_stage::settlement()
            || quote_stage::settlement_quote_supported(q, quote_stage::is_final()))
    {
        let total = hashflow::routing::total(p, q).await;
        if total.is_none() && !quote_stage::settlement() {
            return old;
        }
        total
    } else {
        None
    };
    let Ok(ms) = ctx.int("raindex.quote_timeout_ms") else {
        return old;
    };
    let end = tokio::time::Instant::now() + Duration::from_millis(ms);
    use futures_util::{StreamExt, stream};
    let offers: Vec<_> = rows
        .iter()
        .filter(|v| v.leg.pair() == pair)
        .cloned()
        .collect();
    let candidates = stream::iter(offers)
        .map(|offer| candidate(p, a, s, qty, offer))
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
            p.raindex.save(t);
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
    row: Arc<Live>,
) -> Option<(Quote, Option<Ticket>, Decimal)> {
    let ctx = &p.ctx;
    let generation = ctx.settings.generation();
    p.lag.check(ctx).ok()?;
    let mut pricing = p.quoting();
    direct::taxes(&pricing, a).ok()?;
    let leg = &row.leg;
    let age = Duration::from_millis(ctx.int("raindex.max_age_ms").ok()?);
    let funds = p
        .raindex
        .funds
        .get(p, leg.p.chain, leg.pair().0, leg.target(), age)?;
    if leg.order.owner() == funds.who {
        return None;
    }
    let buy = s == Side::Buy;
    let oi = p.tokens.decimals_known(leg.pair().1)?;
    let decs = if buy {
        (oi, funds.decimals)
    } else {
        (funds.decimals, oi)
    };
    let di = if leg.input == leg.pair().0 {
        funds.decimals
    } else {
        leg.decimals(leg.input, p)?
    };
    let dout = leg.decimals(leg.output, p)?;
    let wanted = direct::ceil_units(qty, decs.0)?;
    if num::to_decimal(wanted, decs.0)? != qty {
        return None;
    }
    let gas_cap = ctx.dec("raindex.gas_budget_native").ok()?;
    let channels = send::select(ctx, &p.stats).ok()?;
    if quote_stage::settlement()
        && (channels.is_empty()
            || channels
                .iter()
                .any(|r| r.method != crate::core::types::SendMethod::Single))
    {
        return None;
    }
    if gas_cap <= Decimal::ZERO
        || funds.native < num::to_units(gas_cap, 18)?
        || send::tip(&channels).is_some()
    {
        return None;
    }
    let mut gas = gas_cap;
    let mut units = 0;
    let mut ticket = None;
    let mut block = p.heads.fresh(ctx).ok()?.number;
    let head = if quote_stage::is_final() {
        Some(white_state::head(ctx, "latest").await.ok()?)
    } else {
        None
    };
    let (cap, ratio, word) = if let Some(h) = head {
        leg.p.verify(ctx, h).await.ok()?;
        if leg.p.version == 3 {
            (row.cap, row.ratio, B256::repeat_byte(255))
        } else {
            leg.current(ctx, h).await?
        }
    } else {
        (row.cap, row.ratio, B256::ZERO)
    };
    let output = if buy {
        wanted
    } else {
        ratio.received(wanted, di, dout)?
    };
    let debit = if buy {
        ratio.paid(wanted, di, dout)?
    } else {
        wanted
    };
    if output.is_zero()
        || debit.is_zero()
        || (output > cap.cap(dout)? && (head.is_none() || row.max_budget.is_none()))
    {
        return None;
    }
    let middle = if buy { debit } else { output };
    let cash_units = match (leg.via, head) {
        (Some(b), Some(h)) => b.official(ctx, h, buy, middle).await?,
        (Some(b), None) => b.estimate(buy, middle)?,
        (None, _) => middle,
    };
    let budget = if buy && head.is_some() {
        row.max_budget
            .map_or(cash_units, |cap| cap.min(funds.spend))
    } else if buy {
        cash_units
    } else {
        wanted
    };
    let limit = if quote_stage::settlement() && head.is_none() {
        funds.allowance
    } else {
        funds.spend
    };
    if budget > limit {
        return None;
    }
    let cash = if let Some(h) = head {
        block = h.number.to();
        let expiry = h
            .timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s").ok()?)?;
        let receive_fixed = buy || leg.p.version <= 5;
        let action = leg.order.take(
            leg.i,
            leg.j,
            if receive_fixed { output } else { debit },
            if receive_fixed { dout } else { di },
            receive_fixed,
            word,
        )?;
        let low = if buy { U256::ONE } else { wanted };
        let payment = Payment::route(
            leg,
            funds.who,
            action.clone(),
            expiry,
            (low, budget, if buy { wanted } else { U256::ONE }),
            middle,
        );
        let (spent, received, gas_used) = payment.measure(ctx, h, expiry).await.ok()?;
        if buy && received != wanted {
            return None;
        }
        pricing.gas = pricing.gas.for_quote(ctx, &p.heads).await.ok()?;
        let bill = exec::cycle_gas::priced(&pricing, funds.who, (leg.target(), payment.data))
            .await
            .ok()?;
        gas = num::to_decimal(
            exec::cycle_gas::simulated(&pricing, bill, gas_used, 0, 0)
                .ok()?
                .wei(),
            18,
        )?;
        if gas > gas_cap || !timely(expiry) {
            return None;
        }
        units = gas_used;
        ticket = Some(Ticket {
            row,
            middle,
            owner: funds.who,
            action,
            spent,
            received,
            expiry,
            qty,
            side: s,
            decs,
            generation,
            fallback_fee: None,
        });
        num::to_decimal(if buy { spent } else { received }, decs.1)?
    } else {
        num::to_decimal(cash_units, decs.1)?
    };
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
            mode: mode(quote_stage::settlement(), ticket.as_ref().map(Ticket::id)),
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
