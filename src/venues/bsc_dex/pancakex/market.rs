//! Сравнение предельных сумм собственного аукциона с результатом существующего маршрута.
pub(in crate::venues::bsc_dex) use super::book::Book;
pub(super) use super::book::{Config, Ticket};
use super::{
    Request,
    client::{self, Query},
};
use crate::{
    core::types::{Asset, DexRoute, Quote, RouteMode, Side},
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        hashflow,
        node::{abi::IERC20, call::canonical},
        num,
        parts::Parts,
        quote::white_state,
        wallet::signer,
    },
};
use alloy_primitives::U256;
use chrono::Utc;
use rust_decimal::Decimal;
pub(super) fn is_route(r: &DexRoute) -> bool {
    matches!(r.mode, RouteMode::PancakeX(_))
}
pub const LABEL: &str = "PancakeSwap X";
async fn price(p: &Parts, asset: &Asset, side: Side, qty: Decimal) -> Option<(Ticket, Quote)> {
    let ctx = &p.ctx;
    let cash = p.tokens.usdt(ctx).ok()?;
    let coin = asset.contract?;
    let buy = side == Side::Buy;
    let (input, output) = if buy { (cash, coin) } else { (coin, cash) };
    let cfg = Config::read(ctx, input, output)?;
    let who = signer::address(ctx).ok()?;
    let h = white_state::head(ctx, "latest").await.ok()?;
    let at = h.selector();
    let (id, od) = tokio::try_join!(
        canonical(ctx, &at, input, IERC20::decimalsCall {}),
        canonical(ctx, &at, output, IERC20::decimalsCall {})
    )
    .ok()?;
    let decimals = if buy { (od, id) } else { (id, od) };
    let size = num::to_units(qty, decimals.0)?;
    if size.is_zero() || num::to_decimal(size, decimals.0)? != qty {
        return None;
    }
    let offer = client::quote(
        ctx,
        &cfg.base,
        Query {
            input,
            output,
            owner: who,
            size,
            buy,
        },
        cfg.bps,
    )
    .await?;
    let req = Request {
        owner: who,
        input,
        output,
        amount: if buy { U256::MAX } else { size },
        minimum: if buy { size } else { U256::ONE },
        exact_output: buy,
        now: h.timestamp.to(),
    };
    let terms = offer.terms(&cfg.p, &req)?;
    let req = Request {
        amount: terms.maximum_input,
        minimum: terms.min_output,
        ..req
    };
    let gross = num::to_decimal(
        if buy {
            terms.maximum_input
        } else {
            cfg.output.guaranteed_net(terms.min_output)
        },
        decimals.1,
    )?;
    let t = Ticket {
        cfg,
        req,
        offer,
        decimals,
        side,
        qty,
        block: h.number.to(),
        deadline: terms.deadline,
    };
    t.eligible(ctx, h).await?;
    let q = Quote {
        venue: ctx.venue(),
        asset: asset.clone(),
        side,
        qty,
        fillable_qty: qty,
        avg_price: gross.checked_div(qty)?,
        costs: vec![],
        route: Some(DexRoute {
            mode: RouteMode::PancakeX(None),
            legs: vec![],
            block: t.block,
            gas_units: 0,
            executable: false,
            exec_note: Some(LABEL.into()),
        }),
        at: Utc::now(),
        signal_route: None,
        pick: Some(LABEL.into()),
    };
    Some((t, q))
}
pub(in crate::venues::bsc_dex) async fn consider(
    p: &Parts,
    a: &Asset,
    side: Side,
    qty: Decimal,
    base: GatewayResult<Quote>,
) -> GatewayResult<Quote> {
    use crate::gateway::{errors::ErrorCode, quote_stage};
    if !p.ctx.flag("pancakex.trading").unwrap_or(false)
        || qty <= Decimal::ZERO
        || !super::execute::channels(p)
        || base
            .as_ref()
            .is_err_and(|e| !matches!(e.code, ErrorCode::NoRoute | ErrorCode::StaleData))
    {
        return base;
    }
    if crate::venues::bsc_dex::quote::direct::taxes(&p.quoting(), a).is_err()
        || crate::venues::bsc_dex::exec::native::is_native(&p.ctx, a)
    {
        return base;
    }
    let Ok(ms) = p.ctx.int("pancakex.quote_budget_ms") else {
        return base;
    };
    let Ok(Some((t, mut q))) =
        tokio::time::timeout(std::time::Duration::from_millis(ms), price(p, a, side, qty)).await
    else {
        return base;
    };
    if let Ok(old) = &base {
        let (Some(before), Some(after)) = (
            hashflow::routing::total(p, old).await,
            hashflow::routing::total(p, &q).await,
        ) else {
            return base;
        };
        if (side == Side::Buy && after >= before) || (side == Side::Sell && after <= before) {
            return base;
        }
    }
    if quote_stage::is_final() || quote_stage::settlement() {
        let id = t.offer.hash;
        p.pancakex.store(t);
        if let Some(r) = q.route.as_mut() {
            r.mode = RouteMode::PancakeX(Some(id));
            r.executable = true;
        }
    }
    Ok(q)
}
