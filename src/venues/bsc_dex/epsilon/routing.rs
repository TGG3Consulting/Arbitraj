//! Поиск денежной доли Epsilon с отдельной оценкой исходного approve, газа и публикации L1.
use super::{model, wire::Saved, *};
use crate::{
    core::types::{Asset, Cost, CostKind, DexRoute, Quote, RouteMode, SendMethod, Side},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        quote_stage,
    },
    venues::bsc_dex::{exec, gas_ticker, num, quote::direct, send},
};
use alloy_primitives::U256;
use rust_decimal::Decimal;
pub(in crate::venues::bsc_dex) struct Ticket {
    pub(super) proof: Saved,
    pub(super) generation: u64,
    pub(super) qty: Decimal,
    pub(super) side: Side,
    pub(super) decs: (u8, u8),
    pub(super) families: [Option<&'static str>; 2],
    pub(super) config: Config,
    pub(in crate::venues::bsc_dex) fallback_fee: Option<Decimal>,
}
pub(in crate::venues::bsc_dex) fn mixed_ticket(route: &DexRoute) -> Option<B256> {
    match route.mode {
        RouteMode::Orderbook(id)
            if route.executable && route.exec_note.as_deref() == Some(LABEL) =>
        {
            Some(id)
        }
        _ => None,
    }
}
pub(in crate::venues::bsc_dex) fn available(p: &Parts, a: &Asset) -> bool {
    enabled(&p.ctx)
        && a.contract.is_some_and(|c| {
            p.tokens
                .usdt(&p.ctx)
                .ok()
                .is_some_and(|q| !p.registry.pair_arcs(c, q).is_empty())
        })
}
struct Search<'a> {
    p: &'a Parts,
    state: &'a State,
    asset: &'a Asset,
    side: Side,
    qty: Decimal,
    raw: U256,
    decs: (u8, u8),
    generation: u64,
    h: Option<white_state::Head>,
    age: Duration,
    gas: Decimal,
}
impl Search<'_> {
    async fn quote(
        &self,
        choice: &model::Choice,
        guarded: bool,
    ) -> Option<(Quote, Option<Ticket>, Decimal)> {
        let p = self.p;
        let ctx = &p.ctx;
        let state = self.state;
        let buy = self.side == Side::Buy;
        let mut order = choice.plan.clone();
        if guarded {
            if !state.guarded {
                return None;
            }
            order.wallet = Some(state.config.wallet.as_ref()?.address);
        } else if !choice.families.into_iter().flatten().all(|id| {
            super::super::pools::exact_wallet_tokens::eligible(ctx, id, order.input, order.output)
        }) {
            return None;
        }
        let target = order.wallet.unwrap_or(order.router);
        let funds = p
            .epsilon
            .funds
            .get(p, state.config.chain, order.input, target, self.age)?;
        let mixed = quote_stage::settlement();
        let available = if mixed && self.h.is_none() {
            funds.allowance
        } else {
            funds.spend
        };
        if order.amount > available || funds.native < num::to_units(self.gas, 18)? {
            return None;
        }
        let mut cost = self.gas;
        let mut units = 0;
        let mut ticket = None;
        let mut cash = if buy { order.amount } else { choice.received };
        let mut block = p.heads.fresh(ctx).ok()?.number;
        if let Some(h) = self.h {
            block = h.number.to();
            order.deadline = h
                .timestamp
                .to::<u64>()
                .checked_add(ctx.int("dex.deadline_s").ok()?)?;
            if !state.accepts(ctx, h, &order, choice.families).await {
                return None;
            }
            let (preview, trial) = order.signed(state.config.chain, funds.who)?;
            let (spent, received, trial_gas) = preview.measure(ctx, h, &trial).await.ok()?;
            if !buy {
                order.minimum = received;
            }
            let (proof, data) = order.signed(state.config.chain, funds.who)?;
            let gas = if trial == data {
                trial_gas
            } else {
                let (x, y, g) = proof.measure(ctx, h, &data).await.ok()?;
                if (x, y) != (spent, received) {
                    return None;
                }
                g
            };
            if spent != order.amount || (buy && received < self.raw) {
                return None;
            }
            let mut q = p.quoting();
            q.gas = q.gas.for_quote(ctx, &p.heads).await.ok()?;
            let bill = exec::cycle_gas::priced(&q, funds.who, (target, data))
                .await
                .ok()?;
            cost = num::to_decimal(
                exec::cycle_gas::simulated(&q, bill, gas, 0, 0).ok()?.wei(),
                18,
            )?;
            if cost > self.gas
                || chrono::Utc::now().timestamp() >= i64::try_from(order.deadline).ok()?
            {
                return None;
            }
            units = gas;
            cash = if buy { spent } else { received };
            ticket = Some(Ticket {
                proof,
                generation: self.generation,
                qty: self.qty,
                side: self.side,
                decs: self.decs,
                families: choice.families,
                config: state.config.clone(),
                fallback_fee: None,
            });
        }
        let money = num::to_decimal(cash, self.decs.1)?;
        let mode = match (mixed, ticket.as_ref()) {
            (true, Some(t)) => RouteMode::Orderbook(t.proof.call.calldata),
            (true, None) => RouteMode::OrderbookIndicative,
            (false, Some(t)) => RouteMode::Rfq(t.proof.call.calldata),
            (false, None) => RouteMode::RfqIndicative,
        };
        let q = Quote {
            venue: ctx.venue(),
            asset: self.asset.clone(),
            side: self.side,
            qty: self.qty,
            fillable_qty: self.qty,
            avg_price: money.checked_div(self.qty)?,
            costs: vec![Cost::new(
                CostKind::Gas,
                cost,
                gas_ticker::read(ctx).ok()?,
                false,
            )],
            route: Some(DexRoute {
                mode,
                legs: vec![],
                block,
                gas_units: units,
                executable: ticket.is_some(),
                exec_note: Some(LABEL.into()),
            }),
            signal_route: None,
            at: chrono::Utc::now(),
            pick: Some(LABEL.into()),
        };
        let total = hashflow::routing::total(p, &q).await?;
        if let Some(t) = ticket.as_mut() {
            t.fallback_fee = Some((total - money).abs())
        }
        Some((q, ticket, total))
    }
}
pub(in crate::venues::bsc_dex) async fn consider(
    p: &Parts,
    a: &Asset,
    s: Side,
    qty: Decimal,
    old: GatewayResult<Quote>,
) -> GatewayResult<Quote> {
    let ctx = &p.ctx;
    if !available(p, a)
        || qty <= Decimal::ZERO
        || old.as_ref().is_err_and(|e| e.code != ErrorCode::NoRoute)
    {
        return old;
    }
    let Some((coin, stable)) = a.contract.zip(p.tokens.usdt(ctx).ok()) else {
        return old;
    };
    let (Some(cd), Some(sd)) = (
        p.tokens.decimals_known(coin),
        p.tokens.decimals_known(stable),
    ) else {
        return old;
    };
    let Some(raw) = direct::ceil_units(qty, cd).filter(|r| num::to_decimal(*r, cd) == Some(qty))
    else {
        return old;
    };
    if p.lag.check(ctx).is_err() || direct::taxes(&p.quoting(), a).is_err() {
        return old;
    }
    let Ok(ms) = ctx.int("epsilon.quote_timeout_ms") else {
        return old;
    };
    let Ok(age) = ctx.int("epsilon.max_age_ms") else {
        return old;
    };
    let age = Duration::from_millis(age);
    let Ok(gas) = ctx.dec("epsilon.gas_budget_native") else {
        return old;
    };
    if gas <= Decimal::ZERO {
        return old;
    }
    let gen0 = ctx.settings.generation();
    let Some(mut state) = p.epsilon.current(p, age) else {
        return old;
    };
    let mixed = quote_stage::settlement();
    let firm = quote_stage::is_final();
    let Ok(ready) = send::select(ctx, &p.stats) else {
        return old;
    };
    if ready.is_empty()
        || send::tip(&ready).is_some()
        || (mixed && ready.iter().any(|r| r.method != SendMethod::Single))
    {
        return old;
    }
    let mut threshold = None;
    if let Ok(q) = &old
        && q.qty == qty
        && q.fillable_qty == qty
        && (!firm || q.route.as_ref().is_none_or(|r| r.executable))
        && (!mixed || quote_stage::settlement_quote_supported(q, firm))
    {
        threshold = hashflow::routing::total(p, q).await;
        if threshold.is_none() && !mixed {
            return old;
        }
    }
    let end = tokio::time::Instant::now() + Duration::from_millis(ms);
    let h = if firm {
        let check = async {
            let h = white_state::head(ctx, "latest").await.ok()?;
            Some((h, state.config.live(ctx, h).await?))
        };
        let Ok(Some((h, fresh))) = tokio::time::timeout_at(end, check).await else {
            return old;
        };
        state = Arc::new(fresh);
        Some(h)
    } else {
        None
    };
    let (input, output) = if s == Side::Buy {
        (stable, coin)
    } else {
        (coin, stable)
    };
    let choices = model::choices(p, &state, input, output, s == Side::Buy, raw);
    let search = Search {
        p,
        state: &state,
        asset: a,
        side: s,
        qty,
        raw,
        decs: (cd, sd),
        generation: gen0,
        h,
        age,
        gas,
    };
    use futures_util::{StreamExt, stream};
    // Одинаковая пара рынков и объём моделируются один раз для каждой отличающейся формы оплаты.
    let mut tasks = Vec::with_capacity(choices.len() * 2);
    for choice in &choices {
        tasks.push(search.quote(choice, false));
        tasks.push(search.quote(choice, true));
    }
    let trials = stream::iter(tasks).buffer_unordered(4);
    tokio::pin!(trials);
    let mut best = None;
    while let Ok(Some(answer)) = tokio::time::timeout_at(end, trials.next()).await {
        if let Some((q, t, n)) = answer
            && threshold.is_none_or(|x| if s == Side::Buy { n < x } else { n > x })
        {
            threshold = Some(n);
            best = Some((q, t))
        }
    }
    if ctx.settings.generation() != gen0 {
        return old;
    }
    if let Some((q, t)) = best {
        if let Some(t) = t {
            p.epsilon.save(t)
        }
        Ok(q)
    } else {
        old
    }
}
