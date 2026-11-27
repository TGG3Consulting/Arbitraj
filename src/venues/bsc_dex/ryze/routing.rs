//! Сравнение Ryze с уже найденным исполнимым маршрутом по общей стоимости; временная книга остаётся только индикативной.
use super::super::{hashflow, parts::Parts, quote::direct};
use super::{
    deployment::Config,
    market::{self, Snapshot},
};
use crate::{
    core::types::{Asset, Quote, Side},
    gateway::errors::{ErrorCode, GatewayResult},
};
use rust_decimal::Decimal;
use std::time::Duration;

pub(in crate::venues::bsc_dex) fn active(p: &Parts) -> bool {
    p.ctx.chain_id() == Some(8453) && p.ctx.flag("ryze.trading").unwrap_or(false)
}
pub(in crate::venues::bsc_dex) async fn consider(
    p: &Parts,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    baseline: GatewayResult<Quote>,
) -> GatewayResult<Quote> {
    let Some(c) = Config::read(&p.ctx) else {
        return baseline;
    };
    let Some(coin) = asset.contract else {
        return baseline;
    };
    if qty <= Decimal::ZERO
        || baseline
            .as_ref()
            .is_err_and(|e| !matches!(e.code, ErrorCode::NoRoute | ErrorCode::StaleData))
        || direct::taxes(&p.quoting(), asset).is_err()
        || super::super::exec::native::is_native(&p.ctx, asset)
    {
        return baseline;
    }
    let firm = crate::gateway::quote_stage::is_final() || crate::gateway::quote_stage::settlement();
    let read = async {
        if firm {
            let s = Snapshot::read(p, c.clone()).await?;
            s.measure(p, coin, side, qty).await
        } else {
            p.ryze.indicative(&c, coin, side, qty)
        }
    };
    let Some(ms) = p.ctx.int("ryze.quote_budget_ms").ok().filter(|v| *v > 0) else {
        return baseline;
    };
    let Ok(Some((ticket, cash))) = tokio::time::timeout(Duration::from_millis(ms), read).await
    else {
        return baseline;
    };
    let Some(mut q) = market::quote(p, asset, &ticket, qty, cash, None) else {
        return baseline;
    };
    if let Ok(base) = &baseline {
        let (Some(old), Some(new)) = (
            hashflow::routing::total(p, base).await,
            hashflow::routing::total(p, &q).await,
        ) else {
            return baseline;
        };
        if (side == Side::Buy && new >= old) || (side == Side::Sell && new <= old) {
            return baseline;
        }
    }
    if firm {
        let id = p.ryze.store(ticket);
        if let Some(r) = &mut q.route {
            r.mode = crate::core::types::RouteMode::Relay(id);
            r.executable = true;
        }
    }
    Ok(q)
}
