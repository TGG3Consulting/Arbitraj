//! Price a single funded ERC20 RFQ at its measured gas and the network's actual fee model.
use super::super::{exec, gas_ticker, num, parts::Parts};
use crate::core::types::{Asset, Cost, CostKind, DexRoute, Quote, RouteMode, Side};
use alloy_primitives::{Bytes, U256};
use rust_decimal::Decimal;
pub(in crate::venues::bsc_dex) struct Terms<'a> {
    pub call: &'a exec::signed_call::Erc20Call,
    pub data: &'a Bytes,
    pub cash: U256,
    pub cash_dec: u8,
    pub block: u64,
    pub gas: u64,
    pub gas_setting: &'static str,
    pub label: &'static str,
}
pub(in crate::venues::bsc_dex) async fn quote(
    p: &Parts,
    a: &Asset,
    s: Side,
    qty: Decimal,
    v: Terms<'_>,
) -> Option<Quote> {
    let ctx = &p.ctx;
    let mut pricing = p.quoting();
    pricing.gas = pricing.gas.for_quote(ctx, &p.heads).await.ok()?;
    let bill = exec::cycle_gas::priced(&pricing, v.call.payer, (v.call.module, v.data.clone()))
        .await
        .ok()?;
    let charge = num::to_decimal(
        exec::cycle_gas::simulated(&pricing, bill, v.gas, 0, 0)
            .ok()?
            .wei(),
        18,
    )?;
    if charge > ctx.dec(v.gas_setting).ok()? {
        return None;
    }
    Some(Quote {
        venue: ctx.venue(),
        asset: a.clone(),
        side: s,
        qty,
        fillable_qty: qty,
        avg_price: num::to_decimal(v.cash, v.cash_dec)?.checked_div(qty)?,
        costs: vec![Cost::new(
            CostKind::Gas,
            charge,
            gas_ticker::read(ctx).ok()?,
            false,
        )],
        route: Some(DexRoute {
            mode: RouteMode::Rfq(v.call.calldata),
            legs: vec![],
            block: v.block,
            gas_units: v.gas,
            executable: true,
            exec_note: Some(v.label.into()),
        }),
        signal_route: None,
        at: chrono::Utc::now(),
        pick: Some(v.label.into()),
    })
}
