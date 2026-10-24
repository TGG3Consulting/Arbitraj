//! Файл: venues/bsc_dex/quote/direct_route.rs
//! Что делает: котировка «прямого» режима (решения 29, 52): лучший одиночный пул с USDT (quote/direct.rs) против лучшей
//! пары пулов через промежуточную монету (quote/via.rs) — берётся больший итог после газа (quote/pick.rs). Пара пулов
//! исполняется одной транзакцией через Universal Router своей DEX, поэтому оба пула — одной DEX с одним и тем же
//! Universal Router, Permit2 и форматом команд.
//! Кто вызывает: quote/mod.rs (режим direct), quote/smart_quote.rs (прямая сделка Uniswap).
//! От чего зависит: quote/direct.rs, quote/via.rs, quote/path.rs, quote/costs.rs, quote/pick.rs, exec/ur.rs (формат).
//! Настройки: dex.universal_router, dex.permit2, dex.universal_router_version [вид пула]; через вызываемые — dex.*.

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{Asset, Quote, RouteMode};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::exec::ur;
use super::super::pools::mids::Mid;
use super::super::pools::registry::Entry;
use super::path::Path;
use super::steps::Names;
use super::via::Via;
use super::{costs, direct, pick};

/// Лучшая прямая котировка. Принимает: расчёт путей, тикеры, монету, количество, пулы монеты с USDT, промежуточные
/// с decimals, фильтр пулов (например только Uniswap), цену газа (wei).
/// Ошибки: NoRoute (причины обоих путей), CoinNotChecked, NotConfigured, BadResponse.
#[allow(clippy::too_many_arguments)]
pub async fn quote(
    v: &Via<'_>,
    names: &Names<'_>,
    asset: &Asset,
    qty: Decimal,
    entries: Vec<Entry>,
    mids: &[(Mid, u8)],
    keep: &(dyn Fn(&Entry) -> bool + Sync),
    gas: U256,
) -> GatewayResult<Quote> {
    let (q, side) = (v.q, v.side);
    let ctx = &q.ctx;
    let build = |p: Path| {
        costs::quote(
            ctx,
            asset,
            side,
            qty,
            &[p],
            RouteMode::Direct,
            v.taxes,
            gas,
            names,
        )
    };
    let mine: Vec<Entry> = entries.into_iter().filter(|e| keep(e)).collect();
    let one = if mine.is_empty() {
        Err(ctx.err(ErrorCode::NoRoute, "пулов с USDT нет"))
    } else {
        direct::best(q, asset, side, qty, mine, v.coin_dec, v.usdt_dec)
            .and_then(|(c, _)| Path::one(ctx, c, side, (v.coin_dec, v.usdt_dec)))
            .and_then(build)
    };
    let two = if mids.is_empty() {
        Err(ctx.err(ErrorCode::NoRoute, "промежуточные монеты не используются"))
    } else {
        let same = |a: &Entry, b: &Entry| one_router(ctx, a, b);
        v.best(mids, qty, keep, &same)
            .map_err(|why| ctx.err(ErrorCode::NoRoute, why.join("; ")))
            .and_then(build)
    };
    match (one, two) {
        (Ok(a), Ok(b)) => Ok(pick::pick(q, a, b, side, v.usdt_dec).await),
        (Ok(a), Err(_)) | (Err(_), Ok(a)) => Ok(a),
        (Err(a), Err(_)) if a.code != ErrorCode::NoRoute => Err(a),
        (Err(a), Err(b)) => Err(ctx.err(
            ErrorCode::NoRoute,
            format!(
                "{}: {}; через промежуточную: {}",
                asset.ticker, a.message, b.message
            ),
        )),
    }
}

/// Можно ли пару пулов исполнить одной транзакцией: одна DEX, один Universal Router, один Permit2, один формат.
/// Возвращает: Ok или причину.
pub fn one_router(ctx: &Ctx, a: &Entry, b: &Entry) -> Result<(), String> {
    let addr = |name: &str, e: &Entry| ctx.dex_addr(name, e.family.id).map_err(|x| x.message);
    let same = a.family.dex == b.family.dex
        && addr("dex.universal_router", a)? == addr("dex.universal_router", b)?
        && addr("dex.permit2", a)? == addr("dex.permit2", b)?
        && ur::format(ctx, a).map_err(|x| x.message)?
            == ur::format(ctx, b).map_err(|x| x.message)?;
    if same {
        Ok(())
    } else {
        Err(format!(
            "{} и {}: разные DEX или Universal Router — одной транзакцией не исполнить",
            a.family.title, b.family.title
        ))
    }
}
