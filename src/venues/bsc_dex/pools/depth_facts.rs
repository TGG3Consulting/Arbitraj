//! Файл: venues/bsc_dex/pools/depth_facts.rs
//! Что делает: пулы площадки для экрана «Монеты» (решение 73, Venue::pool_depths): глубина по направлениям в полосе
//! цены и вердикт (pools/depth_book.rs — посчитанное на текущем состоянии пула, иначе считается сейчас), спит ли пул,
//! достоверен ли, итог сверки с квотером (check/state.rs — по сторонам, cross_check_buy / cross_check_sell), tickSpacing — строкой в details.
//! Кто вызывает: venue.rs (pool_depths).
//! От чего зависит: registry.rs, depth_book.rs, check/state.rs, tokens.rs, ctx.rs.
//! Настройки: через depth_book.rs.

use crate::core::types::{CrossCheck, PoolDepth, SideCheck};
use crate::gateway::errors::GatewayResult;

use super::super::check::state::Checks;
use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::registry::Registry;

/// Все пулы реестра (см. шапку). Ошибки: USDT не известен (venue.quote_asset), узел (decimals USDT).
pub async fn all(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    checks: &Checks,
) -> GatewayResult<Vec<PoolDepth>> {
    let usdt = tokens.usdt(ctx)?;
    let dec = tokens.decimals(ctx, usdt).await?;
    let mut out = Vec::new();
    for r in reg.refs() {
        let Some(e) = reg.get(&r) else { continue };
        // Недостоверный пул — последнее посчитанное (состояние может быть старым); спящий — как есть.
        let got = if e.valid && !e.asleep {
            reg.depth().depth(ctx, reg, tokens, &e, dec)
        } else {
            reg.depth().last(&r).ok_or_else(|| {
                if e.asleep {
                    super::super::quote::select::ASLEEP.to_string()
                } else {
                    "данные пула недостоверны — глубина не считается".to_string()
                }
            })
        };
        let (depth, error) = match got {
            Ok(d) => (Some(d), None),
            Err(why) => (None, Some(why)),
        };
        let q = checks.pool(&r).quoter;
        let side = |x: &super::super::check::quoter_sides::SideCheck| match x {
            None => SideCheck {
                state: CrossCheck::NotChecked,
                reason: None,
            },
            Some(Ok(())) => SideCheck {
                state: CrossCheck::Ok,
                reason: None,
            },
            Some(Err(why)) => SideCheck {
                state: CrossCheck::Failed,
                reason: Some(why.clone()),
            },
        };
        out.push(PoolDepth {
            venue: ctx.venue(),
            pool: r,
            buy_usd: depth.as_ref().map(|d| d.band.buy.round_dp(2)),
            sell_usd: depth.as_ref().map(|d| d.band.sell.round_dp(2)),
            band_pct: depth.as_ref().map(|d| d.band_pct),
            min_usd: depth.as_ref().map(|d| d.min),
            full: depth.as_ref().is_some_and(|d| d.band.full),
            deep_buy: depth.as_ref().is_some_and(|d| d.deep_buy),
            deep_sell: depth.as_ref().is_some_and(|d| d.deep_sell),
            depth_error: error,
            asleep: e.asleep,
            valid: e.valid,
            cross_check_buy: side(&q.buy),
            cross_check_sell: side(&q.sell),
            details: e
                .pool
                .tick_spacing
                .map(|s| format!("tickSpacing {s}"))
                .into_iter()
                .collect(),
        });
    }
    Ok(out)
}
