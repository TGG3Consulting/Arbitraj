//! Выбор одиночного Native RFQ по чистой стоимости на выбранной DEX. Условия подписи остаются в памяти.
use super::{Checked, Profile, settlement};
use crate::{
    core::types::{
        Asset, Cost, CostKind, DexRoute, Quote, RouteMode, Side, TradeRequest, TradeResult,
    },
    gateway::{
        errors::{ErrorCode, GatewayResult},
        quote_stage,
    },
    venues::bsc_dex::{
        ctx::Ctx, exec, gas_ticker, hashflow, num, parts::Parts, quote::direct, send,
    },
};
use alloy_primitives::{Address, B256};
use chrono::Utc;
use rust_decimal::Decimal;
use std::time::Duration;

pub(in crate::venues::bsc_dex) struct Ticket {
    pub(super) checked: Checked,
    pub(super) qty: Decimal,
    pub(super) side: Side,
    pub(super) decs: (u8, u8),
    pub(super) generation: u64,
    pub(in crate::venues::bsc_dex) fallback_fee: Option<Decimal>,
}
impl Ticket {
    pub(super) fn id(&self) -> (Address, B256) {
        let p = &self.checked.firm.proof;
        (p.payer, p.digest)
    }
    pub(super) fn expires(&self) -> u64 {
        self.checked.firm.expiry
    }
    pub(super) fn valid(&self, ctx: &Ctx) -> bool {
        super::enabled(ctx)
            && ctx.settings.generation() == self.generation
            && timely(ctx, self.expires())
    }
}
pub(in crate::venues::bsc_dex) fn timely(ctx: &Ctx, expiry: u64) -> bool {
    ctx.int("native_rfq.min_validity_ms").is_ok_and(|min| {
        expiry.checked_mul(1000).is_some_and(|end| {
            end.saturating_sub(Utc::now().timestamp_millis().max(0) as u64) >= min && min > 0
        })
    })
}
fn pair(p: &Parts, asset: &Asset, side: Side) -> Option<(Address, Address)> {
    let coin = asset.contract?;
    let quote = p.tokens.usdt(&p.ctx).ok()?;
    if coin == quote || exec::native::is_native(&p.ctx, asset) {
        return None;
    }
    Some(if side == Side::Buy {
        (quote, coin)
    } else {
        (coin, quote)
    })
}
pub(in crate::venues::bsc_dex) fn available(p:&Parts,asset:&Asset,side:Side)->bool{
    super::enabled(&p.ctx) && pair(p,asset,side).zip(p.native_quotes.depth(p)).is_some_and(|((a,b),book)|book.contains(a,b))
}
pub(in crate::venues::bsc_dex) fn breaks(
    p: &Parts,
    a: &Asset,
    s: Side,
    max: Decimal,
) -> Vec<Decimal> {
    if !super::enabled(&p.ctx) {
        return Vec::new();
    }
    let Some(((i, o), book)) = pair(p, a, s).zip(p.native_quotes.depth(p)) else {
        return Vec::new();
    };
    book.breaks(i, o, s, max)
}
pub(in crate::venues::bsc_dex) async fn consider(
    p: &Parts,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    old: GatewayResult<Quote>,
) -> GatewayResult<Quote> {
    if !super::enabled(&p.ctx)
        || qty <= Decimal::ZERO
        || old.as_ref().is_err_and(|e| e.code != ErrorCode::NoRoute)
    {
        return old;
    }
    let competitor = old.as_ref().ok().filter(|q| {
        q.fillable_qty == qty
            && q.qty == qty
            && (!quote_stage::is_final() || q.route.as_ref().is_none_or(|r| r.executable))
    });
    let cutoff = if let Some(q) = competitor {
        let Some(n) = hashflow::routing::total(p, q).await else {
            return old;
        };
        Some(n)
    } else {
        None
    };
    let Ok(budget) = p.ctx.int("native_rfq.quote_timeout_ms") else {
        return old;
    };
    let candidate = tokio::time::timeout(
        Duration::from_millis(budget),
        candidate(p, asset, side, qty, cutoff),
    )
    .await
    .ok()
    .flatten();
    candidate.map_or(old, Ok)
}
async fn candidate(
    p: &Parts,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    cutoff: Option<Decimal>,
) -> Option<Quote> {
    let ctx = &p.ctx;
    let generation = ctx.settings.generation();
    let profile = Profile::read(ctx).ok()?;
    let module = profile.module?.address;
    let (i, o) = pair(p, asset, side)?;
    let depth = p.native_quotes.depth(p)?;
    let (quote_cash, minimum_input) = depth.value(i, o, side, qty)?;
    let cap = ctx.dec("native_rfq.gas_budget_native").ok()?;
    if cap <= Decimal::ZERO || send::tip(&send::select(ctx, &p.stats).ok()?).is_some() {
        return None;
    }
    p.lag.check(ctx).ok()?;
    let head = p.heads.fresh(ctx).ok()?;
    let mut pricing = p.quoting();
    direct::taxes(&pricing, asset).ok()?;
    let age = Duration::from_millis(ctx.int("native_rfq.max_age_ms").ok()?);
    let funds = p
        .native_quotes
        .funds
        .get(p, profile.chain, i, module, age)?;
    let di = funds.decimals;

    let buy = side == Side::Buy;
    let amount = direct::ceil_units(if buy { quote_cash } else { qty }, di)?;
    if amount < minimum_input || amount > funds.spend || funds.native < num::to_units(cap, 18)? {
        return None;
    }
    if !buy && num::to_decimal(amount, di)? != qty {
        return None;
    }
    let ticker = gas_ticker::read(ctx).ok()?;
    let mut quote = Quote {
        venue: ctx.venue(),
        asset: asset.clone(),
        side,
        qty,
        fillable_qty: qty,
        avg_price: quote_cash.checked_div(qty)?,
        costs: vec![Cost::new(CostKind::Gas, cap, ticker.clone(), false)],
        route: Some(DexRoute {
            mode: RouteMode::RfqIndicative,
            legs: vec![],
            block: head.number,
            gas_units: 0,
            executable: false,
            exec_note: Some(super::LABEL.into()),
        }),
        signal_route: None,
        at: Utc::now(),
        pick: Some("Native RFQ v6".into()),
    };
    let better =
        |net: Decimal| cutoff.is_none_or(|prior| if buy { net < prior } else { net > prior });
    if !quote_stage::is_final() {
        return better(hashflow::routing::total(p, &quote).await?).then_some(quote);
    }
    let dout=p.tokens.decimals(ctx,o).await.ok()?;
    let mut checked = super::check(ctx, &profile, funds.who, i, o, amount)
        .await
        .ok()?;
    let requested = direct::ceil_units(qty, if buy { dout } else { di })?;
    // Firm API принимает exact-input. На покупку разрешена одна уточнённая котировка по фактическому курсу.
    if buy && checked.received < requested && !checked.received.is_zero() {
        let refined = amount.checked_mul(requested)?.div_ceil(checked.received);
        if refined <= amount || refined > funds.spend {
            return None;
        }
        checked = super::check(ctx, &profile, funds.who, i, o, refined)
            .await
            .ok()?;
    }
    if buy && checked.received < requested || !timely(ctx, checked.firm.expiry) {
        return None;
    }
    let minimum = if buy {
        requested.max(checked.firm.proof.minimum)
    } else {
        checked.firm.proof.minimum
    };
    pricing.gas = pricing.gas.for_quote(ctx, &p.heads).await.ok()?;
    let bill = exec::cycle_gas::priced(&pricing, funds.who, (module, checked.firm.call(minimum)))
        .await
        .ok()?;
    let wei = exec::cycle_gas::simulated(&pricing, bill, checked.gas, 0, 0)
        .ok()?
        .wei();
    let gas = num::to_decimal(wei, 18)?;
    if gas > cap {
        return None;
    }
    // Положительный излишек покупки не увеличивает результат кандидата, пока не продан на второй стороне.
    let cash = if buy {
        num::to_decimal(checked.firm.proof.sold, di)?
    } else {
        num::to_decimal(checked.received, dout)?
    };
    quote.avg_price = cash.checked_div(qty)?;
    quote.costs = vec![Cost::new(CostKind::Gas, gas, ticker, false)];
    quote.route = Some(DexRoute {
        mode: RouteMode::Rfq(checked.firm.proof.digest),
        legs: vec![],
        block: checked.head.number.to(),
        gas_units: checked.gas,
        executable: true,
        exec_note: Some(super::LABEL.into()),
    });
    let net = hashflow::routing::total(p, &quote).await?;
    if !better(net) || ctx.settings.generation() != generation || !timely(ctx, checked.firm.expiry)
    {
        return None;
    }
    let decs = if buy { (dout, di) } else { (di, dout) };
    p.native_quotes.save(Ticket {
        checked,
        qty,
        side,
        decs,
        generation,
        fallback_fee: Some((net - cash).abs()),
    });
    Some(quote)
}
pub(in crate::venues::bsc_dex) async fn trade(
    p: &Parts,
    req: TradeRequest,
    t: &Ticket,
) -> GatewayResult<TradeResult> {
    settlement::trade(p, req, t).await
}
