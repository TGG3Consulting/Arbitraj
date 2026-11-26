//! Оценка объёма по книге выбранного строителя и перепроверка кошелька, предела и полной стоимости в его слоте.
use super::super::{
    gas_ticker, hashflow, num,
    parts::Parts,
    send::{tx, tx_1559::Fees},
};
use super::{
    Head,
    indicative::{Candidate, candidate},
    markets::Books,
    slot::{self, Proof},
};
use crate::core::types::{Asset, Cost, CostKind, Quote, RouteMode, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::{
    primitives::{Address, B256, U64, U256, keccak256},
    sol_types::SolValue,
};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

pub(super) struct Prepared {
    pub book: Arc<Books>,
    pub proof: Proof,
    pub base: U256,
    pub fees: Fees,
    pub gas_limit: u64,
    pub spent: U256,
    pub received: U256,
    pub decs: (u8, u8),
}
type PerWallet = HashMap<(Address, B256), Arc<Prepared>>;
#[derive(Default, Clone)]
pub(super) struct Tickets(Arc<Mutex<PerWallet>>);
impl Tickets {
    fn save(&self, q: Prepared) -> B256 {
        let id = keccak256(
            (
                q.proof.calldata(),
                q.proof.payer,
                q.proof.executor,
                q.proof.builder.clone(),
                q.fees.max_fee,
                q.fees.tip,
            )
                .abi_encode(),
        );
        let mut m = self.0.lock().unwrap_or_else(|x| x.into_inner());
        m.retain(|_, v| v.book.current());
        m.insert((q.proof.payer, id), Arc::new(q));
        id
    }
    pub(super) fn take(&self, who: Address, id: B256) -> Option<Arc<Prepared>> {
        self.0
            .lock()
            .unwrap_or_else(|x| x.into_inner())
            .remove(&(who, id))
    }
}
pub(in crate::venues::bsc_dex) fn active(p: &Parts) -> bool {
    p.ctx.chain_id() == Some(1)
        && p.ctx.flag("fermi.trading").unwrap_or(false)
        && p.slot_markets.current(p).is_some()
}
pub(in crate::venues::bsc_dex) fn breaks(
    p: &Parts,
    asset: &Asset,
    side: Side,
    max: Decimal,
) -> Vec<Decimal> {
    let f = || {
        p.slot_markets
            .current(p)?
            .sides
            .get(&(asset.contract?, side == Side::Buy))
            .map(|levels| levels.breaks(max))
    };
    f().unwrap_or_default()
}
async fn firm(p: &Parts, c: Candidate) -> Option<(Prepared, Quote)> {
    let b = c.book;
    let h = &b.head;
    let f = &b.frame;
    let ctx = &p.ctx;
    let who = c.who;
    let (fees, base) = p.gas.fees(ctx, &p.heads).await.ok()?;
    let executor = b.config.executor;
    let (s, r) = slot::amounts(ctx, h, f, executor, c.input, c.output, c.amount)
        .await
        .ok()?;
    let leg = keccak256((h.hash, who, c.input, c.output, c.amount).abi_encode());
    let proof = Proof {
        builder: b.config.builder.name().into(),
        parent: h.hash,
        target: f.target,
        timestamp: f.timestamp,
        executor,
        payer: who,
        input: c.input,
        output: c.output,
        amount: c.amount,
        limit: if c.amount.is_negative() { s } else { r },
        leg,
    };
    let (spent, received, gas) = slot::simulate(ctx, h, f, &proof, Some((fees.max_fee, fees.tip)))
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
    let latest: Head = super::rpc::call(ctx, "eth_getBlockByNumber", json!(["latest", false]))
        .await
        .ok()?;
    if latest.hash != h.hash
        || !b.current()
        || super::markets::Config::read(p).as_ref() != Some(&b.config)
    {
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
pub(in crate::venues::bsc_dex) async fn consider(
    p: &Parts,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    baseline: GatewayResult<Quote>,
) -> GatewayResult<Quote> {
    if p.ctx.chain_id() != Some(1)
        || !p.ctx.flag("fermi.trading").unwrap_or(false)
        || baseline
            .as_ref()
            .is_err_and(|e| e.code != ErrorCode::NoRoute)
    {
        return baseline;
    }
    let Some(c) = candidate(p, asset, side, qty) else {
        return baseline;
    };
    let old = if let Ok(b) = &baseline
        && b.fillable_qty == qty
        && b.qty == qty
        && (!crate::gateway::quote_stage::is_final()
            || b.route.as_ref().is_none_or(|r| r.executable))
    {
        let Some(cost) = hashflow::routing::total(p, b).await else {
            return baseline;
        };
        Some(cost)
    } else {
        None
    };
    let improves =
        |net: Decimal| old.is_none_or(|v| if side == Side::Buy { net < v } else { net > v });
    let Some(pre) = hashflow::routing::total(p, &c.quote).await else {
        return baseline;
    };
    if !improves(pre) {
        return baseline;
    }
    if !crate::gateway::quote_stage::is_final() {
        return Ok(c.quote);
    }
    if p.ctx.account.is_none() {
        return baseline;
    }
    let timeout = Duration::from_millis(c.book.config.timeout);
    let Ok(Some((prepared, mut quote))) = tokio::time::timeout(timeout, firm(p, c)).await else {
        return baseline;
    };
    if !hashflow::routing::total(p, &quote)
        .await
        .is_some_and(improves)
        || !prepared.book.current()
    {
        return baseline;
    }
    let id = p.slot_markets.tickets.save(prepared);
    if let Some(route) = quote.route.as_mut() {
        route.mode = RouteMode::Slot(id);
    }
    Ok(quote)
}
