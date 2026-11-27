//! Сравнение публичного маршрута OpenOcean в двух формах со штатными пулами в той же валюте.
use super::{
    ocean_api::Client,
    ocean_role::{self, Roles},
    ocean_wire::Ocean,
    routing::Ticket,
    wire::Plan,
    *,
};
use crate::{
    core::types::{Asset, Cost, CostKind, DexRoute, Quote, RouteMode, SendMethod, Side},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        quote_stage,
    },
    venues::bsc_dex::{exec, gas_ticker, node::rpc, num, quote::direct, send},
};
use alloy_primitives::{Address, U256, aliases::U24};
use rust_decimal::Decimal;
use serde_json::json;

pub(super) const DIRECT: &str = "OpenOcean";
pub(super) fn configured(ctx: &Ctx) -> bool {
    enabled(ctx) && ctx.text("epsilon.openocean_url").is_ok() && Roles::read(ctx).is_some()
}
struct Compare<'a> {
    p: &'a Parts,
    role: &'a Roles,
    client: Client<'a>,
    asset: &'a Asset,
    coin: Address,
    stable: Address,
    side: Side,
    qty: Decimal,
    raw: U256,
    decs: (u8, u8),
    h: white_state::Head,
    age: Duration,
    gas: Decimal,
    generation: u64,
    firm: bool,
    mixed: bool,
}
impl Compare<'_> {
    async fn route(
        &self,
        nested: bool,
        fee: u16,
        seed: U256,
    ) -> Option<(Quote, Option<Ticket>, Decimal)> {
        let buy = self.side == Side::Buy;
        let (input, output) = if buy {
            (self.stable, self.coin)
        } else {
            (self.coin, self.stable)
        };
        let (r, ctx) = (self.role, &self.p.ctx);
        let funds = self
            .p
            .epsilon
            .funds
            .get(self.p, r.chain, input, r.wallet.address, self.age)?;
        let spend = if self.mixed && !self.firm {
            funds.allowance
        } else {
            funds.spend
        };
        if funds.native < num::to_units(self.gas, 18)? {
            return None;
        }
        let mut amount = seed;
        let mut pick = None;
        // На покупке количество входа уточняется ценой в нужном направлении, включая округление сбора.
        for _ in 0..3 {
            if amount.is_zero() || amount > spend {
                return None;
            }
            let after =
                amount.checked_sub(amount.checked_mul(U256::from(fee))? / U256::from(10000))?;
            let offer = self
                .client
                .request(
                    input,
                    output,
                    after,
                    self.firm.then_some(if nested {
                        r.venue.address
                    } else {
                        r.wallet.address
                    }),
                )
                .await?;
            let output_quote = offer.out_amount.parse::<U256>().ok()?;
            if output_quote.is_zero() {
                return None;
            }
            if !buy || output_quote >= self.raw {
                pick = Some((offer, output_quote));
                break;
            }
            let next = crate::venues::bsc_dex::math::full_math::mul_div_up(
                amount,
                self.raw,
                output_quote,
            )?;
            amount = next.max(amount.checked_add(U256::ONE)?);
        }
        let (offer, mut got) = pick?;
        let mut native = self.gas;
        let mut gas_units = 0;
        let mut ticket = None;
        let mut deadline = self
            .h
            .timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s").ok()?)?;
        if let Some(exp) = offer.rfq_deadline.filter(|x| *x > 0) {
            deadline = deadline.min(exp)
        }
        let label = if nested { LABEL } else { DIRECT };
        if self.firm {
            if offer.chain_id != Some(r.chain)
                || offer.from
                    != Some(if nested {
                        r.venue.address
                    } else {
                        r.wallet.address
                    })
                || offer.to != Some(r.ocean.address)
                || offer.value.as_deref() != Some("0")
                || offer.min_out_amount?.parse::<U256>().ok()?.is_zero()
            {
                return None;
            }
            let mut plan = Plan {
                router: r.venue.address,
                providers: [Address::ZERO; 2],
                pools: [Address::ZERO; 2],
                tiers: [U24::ZERO; 2],
                input,
                output,
                amount,
                first: U256::ZERO,
                fee_bps: fee,
                minimum: if buy { self.raw } else { U256::ONE },
                deadline,
                wallet: Some(r.wallet.address),
                ocean: Some(Ocean {
                    proxy: r.ocean.address,
                    implementation: r.code(),
                    executor: r.executor.address,
                    nested,
                    funding: r.funding.clone().map(Box::new),
                    payload: offer.data?,
                }),
            };
            if !ocean_role::verified(ctx, self.h, &plan).await {
                return None;
            }
            let (test, initial) = plan.signed(r.chain, funds.who)?;
            let (spent, net, measured) = test.measure(ctx, self.h, &initial).await.ok()?;
            if spent != amount || buy && net < self.raw {
                return None;
            }
            if !buy {
                plan.minimum = net;
            }
            let (proof, body) = plan.signed(r.chain, funds.who)?;
            gas_units = if initial == body {
                measured
            } else {
                let (x, y, g) = proof.measure(ctx, self.h, &body).await.ok()?;
                if (x, y) != (spent, net) {
                    return None;
                }
                g
            };
            let mut q = self.p.quoting();
            q.gas = q.gas.for_quote(ctx, &self.p.heads).await.ok()?;
            let b = exec::cycle_gas::priced(&q, funds.who, (r.wallet.address, body))
                .await
                .ok()?;
            native = num::to_decimal(
                exec::cycle_gas::simulated(&q, b, gas_units, 0, 0)
                    .ok()?
                    .wei(),
                18,
            )?;
            if native > self.gas
                || chrono::Utc::now().timestamp() >= i64::try_from(deadline).ok()?
            {
                return None;
            }
            got = net;
            ticket = Some(Ticket {
                proof,
                generation: self.generation,
                qty: self.qty,
                side: self.side,
                decs: self.decs,
                families: [None, None],
                config: None,
                fallback_fee: None,
            });
        }
        let cash = num::to_decimal(if buy { amount } else { got }, self.decs.1)?;
        let mode = match (self.mixed, ticket.as_ref()) {
            (true, Some(t)) => RouteMode::Orderbook(t.proof.call.calldata),
            (false, Some(t)) => RouteMode::Rfq(t.proof.call.calldata),
            (true, None) => RouteMode::OrderbookIndicative,
            (false, None) => RouteMode::RfqIndicative,
        };
        let q = Quote {
            venue: ctx.venue(),
            side: self.side,
            asset: self.asset.clone(),
            qty: self.qty,
            fillable_qty: self.qty,
            avg_price: cash.checked_div(self.qty)?,
            costs: vec![Cost::new(
                CostKind::Gas,
                native,
                gas_ticker::read(ctx).ok()?,
                false,
            )],
            route: Some(DexRoute {
                mode,
                legs: vec![],
                block: self.h.number.to(),
                gas_units,
                executable: ticket.is_some(),
                exec_note: Some(label.into()),
            }),
            signal_route: None,
            pick: Some(label.into()),
            at: chrono::Utc::now(),
        };
        let total = hashflow::routing::total(self.p, &q).await?;
        if let Some(t) = ticket.as_mut() {
            t.fallback_fee = Some((cash - total).abs());
        }
        Some((q, ticket, total))
    }
}

pub(super) async fn consider(
    p: &Parts,
    a: &Asset,
    s: Side,
    qty: Decimal,
    old: GatewayResult<Quote>,
) -> GatewayResult<Quote> {
    let ctx = &p.ctx;
    if !configured(ctx)
        || qty <= Decimal::ZERO
        || old.as_ref().is_err_and(|e| e.code != ErrorCode::NoRoute)
    {
        return old;
    }
    let Ok(ms) = ctx.int("epsilon.quote_timeout_ms") else {
        return old;
    };
    let end = tokio::time::Instant::now() + Duration::from_millis(ms);
    let mut best = None;
    let work = async {
        let role = Roles::read(ctx)?;
        let coin = a.contract?;
        let stable = p.tokens.usdt(ctx).ok()?;
        if coin == stable {
            return None;
        }
        let decs = (
            p.tokens.decimals_known(coin)?,
            p.tokens.decimals_known(stable)?,
        );
        let raw =
            direct::ceil_units(qty, decs.0).filter(|x| num::to_decimal(*x, decs.0) == Some(qty))?;
        let generation = ctx.settings.generation();
        let firm = quote_stage::is_final();
        let mixed = quote_stage::settlement();
        let channels = send::select(ctx, &p.stats).ok()?;
        if channels.is_empty()
            || send::tip(&channels).is_some()
            || (mixed && channels.iter().any(|r| r.method != SendMethod::Single))
        {
            return None;
        }
        p.lag.check(ctx).ok()?;
        direct::taxes(&p.quoting(), a).ok()?;
        let allowance = ctx.dec("epsilon.gas_budget_native").ok()?;
        if allowance <= Decimal::ZERO {
            return None;
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
                return None;
            }
        }
        let age = Duration::from_millis(ctx.int("epsilon.max_age_ms").ok()?);
        let (h, price) = tokio::try_join!(
            white_state::head(ctx, "latest"),
            rpc::call::<U256>(ctx, "eth_gasPrice", json!([]))
        )
        .ok()?;
        let client = Client::new(ctx, price)?;
        let seed = if s == Side::Buy {
            let quote = client.request(coin, stable, raw, None).await?;
            quote.out_amount.parse::<U256>().ok()?
        } else {
            raw
        };
        let book = Compare {
            p,
            role: &role,
            client,
            asset: a,
            coin,
            stable,
            side: s,
            qty,
            raw,
            decs,
            h,
            age,
            gas: allowance,
            generation,
            firm,
            mixed,
        };
        let nested = async {
            let fee = role.fee(ctx, h).await?;
            book.route(true, fee, seed).await
        };
        use futures_util::{StreamExt, stream::FuturesUnordered};
        let mut offers = FuturesUnordered::new();
        use std::{future::Future, pin::Pin};
        type Result = Option<(Quote, Option<Ticket>, Decimal)>;
        offers.push(Box::pin(nested) as Pin<Box<dyn Future<Output = Result> + Send + '_>>);
        offers.push(Box::pin(book.route(false, 0, seed)));
        while let Some(candidate) = offers.next().await {
            if let Some((q, t, n)) = candidate
                && threshold.is_none_or(|v| if s == Side::Buy { n < v } else { n > v })
            {
                threshold = Some(n);
                best = Some((q, t, generation));
            }
        }
        Some(())
    };
    let _ = tokio::time::timeout_at(end, work).await;
    if let Some((q, t, g)) = best
        && g == ctx.settings.generation()
    {
        if let Some(t) = t {
            p.epsilon.save(t)
        }
        Ok(q)
    } else {
        old
    }
}
