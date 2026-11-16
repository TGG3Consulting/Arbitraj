//! Файл: venues/bsc_dex/quote/breaks.rs
//! Что делает: решение 344 (задача t159) — изломы кривых пулов монеты по объёму монеты для подбора объёма ядра
//! (контракт quote_breaks, core/size_breaks.rs): по каждому пулу монеты с USDT и с активными промежуточными монетами
//! (те же пары, что у маршрутов: quote/mod.rs entries, quote/via_pools.rs, quote/graph_route.rs allowed) — объёмы, на
//! которых у пула меняется кусок кривой (quote/breaks_pool.rs). Изломы пулов «промежуточная — USDT» в монету не
//! переводятся (их место на кривой монеты зависит от первого шага) — не отдаются.
//! Только память реестра: ни одного запроса к узлу, кроме decimals монеты (кэш tokens.rs).
//! Кто вызывает: venue.rs (контракт quote_breaks).
//! От чего зависит: quote/breaks_pool.rs, pools/registry.rs, pools/mids.rs, tokens.rs, num.rs.
//! Настройки: dex.intermediate_enabled (через mids.active); число изломов на пул — от ядра (size.pool_breaks_max).

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{Asset, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::num;
use super::Quoting;

/// Что делает: изломы пулов монеты. Принимает: окружение котировки, монету, сторону (продажа — монета на входе пула,
/// покупка — на выходе), наибольший объём (монет), сколько изломов на пул. Возвращает: объёмы монеты по возрастанию
/// без повторов. Ошибки: NoRoute — нет адреса монеты; NotConfigured — тикер USDT; узел — decimals монеты.
pub async fn breaks(
    q: &Quoting,
    asset: &Asset,
    side: Side,
    max_qty: Decimal,
    per_pool: u32,
) -> GatewayResult<Vec<Decimal>> {
    let ctx = &q.ctx;
    let coin = asset.bsc_contract.ok_or_else(|| {
        ctx.err(
            ErrorCode::NoRoute,
            format!("{}: нет адреса контракта в BSC", asset.ticker),
        )
    })?;
    let usdt = q.tokens.usdt(ctx)?;
    let dec = q.tokens.decimals(ctx, coin).await?;
    let max = num::to_units(max_qty, dec).unwrap_or(U256::MAX);
    let mut counters = vec![usdt];
    counters.extend(q.mids.active(ctx).into_iter().map(|m| m.address));
    let sell = side == Side::Sell;
    let mut out: Vec<Decimal> = Vec::new();
    for c in counters {
        for e in q.reg.by_pair(coin, c) {
            let units = super::breaks_pool::pool(&e, coin, sell, max, per_pool);
            out.extend(units.into_iter().filter_map(|u| num::to_decimal(u, dec)));
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}
