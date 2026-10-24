//! Файл: venues/bsc_dex/quote/smart_quote.rs
//! Что делает: котировка режима «smart» (решение 38): умный маршрут по пулам DEX умного роутера (PancakeSwap —
//! dex.universal_router = smart.universal_router) и путям через промежуточные монеты (quote/smart.rs, решение 52)
//! против прямой сделки в пулах остальных DEX (Uniswap и др.) — лучший пул или пара пулов через промежуточную
//! (quote/direct_route.rs) — берётся больший итог (quote/pick.rs). Монета только с пулами других DEX торгуется
//! напрямую; только с пулами умного роутера — умным маршрутом. Затраты умного маршрута — затраты его долей по шагам (quote/costs.rs).
//! Кто вызывает: quote/mod.rs.
//! От чего зависит: quote/smart.rs, quote/direct_route.rs, quote/costs.rs, quote/pick.rs, quote/via.rs, pools/families.rs.
//! Настройки: через вызываемые — smart.*, dex.* (см. их шапки).

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{Asset, Quote, RouteMode};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::pools::families::smart_ok;
use super::super::pools::mids::Mid;
use super::super::pools::registry::Entry;
use super::steps::Names;
use super::via::Via;
use super::{costs, direct_route, pick, smart};

/// Котировка режима «smart». Принимает: расчёт путей, тикеры, монету, количество, пулы монеты с USDT, промежуточные
/// с decimals, цену газа (wei). Ошибки: NotConfigured, CoinNotChecked, NoRoute (причины обоих путей), BadResponse.
#[allow(clippy::too_many_arguments)]
pub async fn quote(
    v: &Via<'_>,
    names: &Names<'_>,
    asset: &Asset,
    qty: Decimal,
    entries: Vec<Entry>,
    mids: &[(Mid, u8)],
    gas: U256,
) -> GatewayResult<Quote> {
    let ctx = &v.q.ctx;
    let smart = smart::best(v, &asset.ticker, qty, &entries, mids).and_then(|paths| {
        costs::quote(
            ctx,
            asset,
            v.side,
            qty,
            &paths,
            RouteMode::Smart,
            v.taxes,
            gas,
            names,
        )
    });
    let uni = |e: &Entry| !smart_ok(ctx, e.family);
    let direct = direct_route::quote(v, names, asset, qty, entries, mids, &uni, gas).await;
    match (smart, direct) {
        (Ok(a), Ok(b)) => Ok(pick::pick(v.q, a, b, v.side, v.usdt_dec).await),
        (Ok(a), Err(_)) | (Err(_), Ok(a)) => Ok(a),
        (Err(a), Err(_)) if a.code != ErrorCode::NoRoute => Err(a),
        (Err(a), Err(b)) => Err(ctx.err(
            ErrorCode::NoRoute,
            format!("{}; прямая сделка (другие DEX): {}", a.message, b.message),
        )),
    }
}
