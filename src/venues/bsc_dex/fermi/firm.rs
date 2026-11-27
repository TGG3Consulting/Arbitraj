//! Готовит оплачиваемую котировку выбранного слота с полной стоимостью газа до подписи.
use super::super::{gas_ticker, num, parts::Parts, send::tx};
use super::{
    indicative::Candidate,
    routing::Prepared,
    slot::{self, Proof},
};
use crate::core::types::{Cost, CostKind, Quote, Side};
use alloy::{
    primitives::{U64, U256, keccak256},
    sol_types::SolValue,
};
use chrono::Utc;
pub(super) async fn firm(p: &Parts, c: Candidate) -> Option<(Prepared, Quote)> {
    let b = c.book.clone();
    let h = &b.head;
    let f = &b.frame;
    let ctx = &p.ctx;
    let who = c.who;
    let (fees, base) = p.gas.fees(ctx, &p.heads).await.ok()?;
    let executor = b.config.executor;
    let leg = keccak256((h.hash, who, c.input, c.output, c.amount).abi_encode());
    let proof = if let Some(t) = b.prop {
        super::prop_slot::candidate(ctx, &c, t, leg).await?
    } else {
        let (s, r) = slot::amounts(ctx, h, f, executor, c.input, c.output, c.amount)
            .await
            .ok()?;
        Proof {
            prop: None,
            builder: b.config.builder.name().into(),
            parent: h.hash,
            target: f.target,
            timestamp: f.timestamp,
            executor,
            router: Some(f.deployment.router),
            payer: who,
            input: c.input,
            output: c.output,
            amount: c.amount,
            limit: if c.amount.is_negative() { s } else { r },
            leg,
        }
    };
    let (spent, received, gas) =
        super::slot_config::simulate(ctx, &b, &proof, (fees.max_fee, fees.tip))
            .await
            .ok()?;
    let limit = tx::with_margin(
        ctx,
        U64::from(gas),
        ctx.dec("send.gas_limit_margin_pct").ok()?,
    )
    .ok()?;
    let reserve = U256::from(fees.max_fee).checked_mul(U256::from(limit))?;
    if reserve > num::to_units(b.config.gas, 18)? {
        return None;
    }
    if !super::slot_config::fresh(p, &b).await {
        return None;
    }
    let mut quote = c.quote;
    let gross = num::to_decimal(
        if quote.side == Side::Buy {
            spent
        } else {
            received
        },
        c.decs.1,
    )?;
    if quote.side == Side::Buy
        && crate::gateway::quote_stage::buy_budget(&ctx.venue().0, b.config.cash)
            .is_some_and(|cap| gross > cap)
    {
        return None;
    }
    quote.avg_price = gross.checked_div(quote.qty)?;
    // До подписания допускаем стоимость газа вплоть до потолка типа 2; отдельного перевода чаевых нет.
    let cost = num::to_decimal(U256::from(fees.max_fee).checked_mul(U256::from(gas))?, 18)?;
    quote.costs = vec![Cost::new(
        CostKind::Gas,
        cost,
        gas_ticker::read(ctx).ok()?,
        false,
    )];
    if let Some(route) = quote.route.as_mut() {
        route.gas_units = gas;
        route.executable = true;
        route.exec_note = None;
    }
    quote.at = Utc::now();
    Some((
        Prepared {
            book: b,
            proof,
            fees,
            base,
            gas_limit: limit,
            spent,
            received,
            decs: c.decs,
        },
        quote,
    ))
}
