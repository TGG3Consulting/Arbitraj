//! Файл: venues/bsc_dex/exec/dry_paths.rs
//! Что делает: части сборки сухой сделки DEX (решение 121, C4): доли маршрута по живому состоянию пулов (как
//! exec/trade_smart.rs и exec/bundle_trade.rs) и предторговая верификация графового маршрута (как
//! exec/verify_gate.rs::gate — quote/verify::run + verify_gate::outcome, но без уведомления exec.verified /
//! exec.verify_rejected: сухая сделка уведомлений сделки не шлёт).
//! Кто вызывает: exec/dry_build.rs.
//! От чего зависит: exec/{live, trade (sides), verify_gate}.rs, quote/{verify, via, steps, path, direct}.rs, send/mod.rs.
//! Настройки: через вызываемые — exec.dex_slippage_pct, venue.quote_asset, verify.*.

use rust_decimal::Decimal;

use crate::core::types::{DexRoute, Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::quote::path::Path;
use super::super::quote::steps::Names;
use super::super::quote::verify;
use super::super::quote::via::Via;
use super::super::quote::{Quoting, direct};
use super::super::send;
use super::trade::sides;
use super::{live, verify_gate};

/// Доли по живому состоянию — как в exec/trade_smart.rs: продажа делится по доле входа, покупка — по ожидаемым
/// монетам доли; последняя доля получает остаток. Возвращает: (путь, монет доли). Ошибки: как у exec/live.rs.
pub async fn live_paths(
    v: &Via<'_>,
    req: &TradeRequest,
    route: &DexRoute,
    direct_mode: bool,
) -> GatewayResult<Vec<(Path, Decimal)>> {
    let ctx = &v.q.ctx;
    let weights: Vec<Decimal> = route
        .legs
        .iter()
        .map(|l| {
            if req.side == Side::Buy {
                l.expected_out
            } else {
                l.share
            }
        })
        .collect();
    let total: Decimal = weights.iter().sum();
    if route.legs.is_empty() || total <= Decimal::ZERO {
        return Err(ctx.err(ErrorCode::NoRoute, "маршрут без долей"));
    }
    let mut out = Vec::with_capacity(route.legs.len());
    let mut left = req.qty;
    for (i, (leg, w)) in route.legs.iter().zip(&weights).enumerate() {
        let qty = if i + 1 == route.legs.len() {
            left
        } else {
            (req.qty * w / total).min(left)
        };
        left -= qty;
        out.push((live::path(v, &leg.hops, qty, direct_mode).await?, qty));
    }
    Ok(out)
}

/// Предторговая верификация графового маршрута — как exec/verify_gate.rs::gate, без уведомления. Возвращает: свежие
/// минимумы по ногам. Ошибки: QuoteMismatch / StaleData (Reject), NotConfigured (exec.dex_slippage_pct, verify.*), узел.
pub async fn verified(
    q: &Quoting,
    req: &TradeRequest,
    route: &DexRoute,
) -> GatewayResult<Vec<Decimal>> {
    let ctx = &q.ctx;
    let (coin, usdt, decs) = sides(q, req).await?;
    let taxes = direct::taxes(q, &req.asset)?;
    let slip_pct = ctx.dec("exec.dex_slippage_pct")?;
    let plan = verify_gate::plan_quote(ctx, req, route, slip_pct)?;
    let gas = q
        .gas
        .wei(ctx, &q.heads)
        .await?
        .max(send::planned_floor(ctx)?);
    let usdt_ticker = ctx.text("venue.quote_asset")?;
    let v = Via {
        q,
        coin,
        usdt,
        side: req.side,
        taxes: &taxes,
        coin_dec: decs.0,
        usdt_dec: decs.1,
    };
    let names = Names {
        usdt: (usdt, &usdt_ticker),
        coin: (coin, &req.asset.ticker),
        mids: &q.mids,
    };
    let verdict = verify::run(&v, &names, &plan, gas).await?;
    Ok(verify_gate::outcome(ctx, verdict)?.amount_out_min)
}
