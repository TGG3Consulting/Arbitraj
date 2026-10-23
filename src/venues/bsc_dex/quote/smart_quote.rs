//! Файл: venues/bsc_dex/quote/smart_quote.rs
//! Что делает: котировка режима «smart» (решение 38): умный маршрут по пулам PancakeSwap (quote/smart.rs) против
//! прямой сделки в лучшем пуле Uniswap (quote/direct.rs) — берётся больший итог: сначала больше исполнимое
//! количество, затем лучше итог в USDT после газа (продажа — больше получим, покупка — меньше отдадим).
//! Монета только с пулами Uniswap торгуется напрямую; только с PancakeSwap — умным маршрутом.
//! Затраты умного маршрута — сумма затрат его долей (quote/costs.rs по каждой доле).
//! Кто вызывает: quote/mod.rs.
//! От чего зависит: quote/smart.rs, quote/direct.rs, quote/costs.rs, quote/pool_quote.rs, pools/registry.rs, tokens.rs.
//! Настройки: через вызываемые — smart.*, dex.* (см. их шапки).
//!
//! Газ в USDT для сравнения — продажей суммы газа в BNB в пул монеты «BNB» из реестра (если BNB среди монет
//! подписки); нет такого пула — сравнение без газа (газ BSC на долю — доли цента; вопрос Тиграну в Issue #7).

use alloy_primitives::U256;
use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Asset, Cost, DexRoute, Quote, RouteMode, Side, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::num;
use super::super::pools::families::PANCAKESWAP;
use super::super::pools::registry::Entry;
use super::super::wallet::balances::BNB;
use super::direct::{self, Candidate};
use super::{Quoting, costs, pool_quote, smart};

/// Котировка режима «smart». Принимает: монету, сторону, количество, пулы монеты, налоги, (decimals монеты, USDT),
/// цену газа (wei), тикер USDT. Ошибки: NotConfigured, CoinNotChecked, NoRoute (причины обоих путей), BadResponse.
#[allow(clippy::too_many_arguments)]
pub async fn quote(
    q: &Quoting,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    entries: Vec<Entry>,
    t: &TokenTaxes,
    decs: (u8, u8),
    gas: U256,
    usdt: &str,
) -> GatewayResult<Quote> {
    let ctx = &q.ctx;
    let smart = smart::best(q, &asset.ticker, side, qty, &entries, t, decs)
        .and_then(|legs| merge(q, asset, side, qty, &legs, t, gas, usdt));
    let uni: Vec<Entry> = entries
        .into_iter()
        .filter(|e| e.family.dex != PANCAKESWAP)
        .collect();
    let direct = if uni.is_empty() {
        Err(ctx.err(ErrorCode::NoRoute, "пулов Uniswap нет"))
    } else {
        direct::best(q, asset, side, qty, uni, decs.0, decs.1)
            .and_then(|(c, _)| costs::quote(ctx, asset, side, qty, &c, t, gas, usdt))
    };
    match (smart, direct) {
        (Ok(a), Ok(b)) => Ok(pick(q, a, b, side, decs.1).await),
        (Ok(a), Err(_)) | (Err(_), Ok(a)) => Ok(a),
        (Err(a), Err(_)) if a.code != ErrorCode::NoRoute => Err(a),
        (Err(a), Err(b)) => Err(ctx.err(
            ErrorCode::NoRoute,
            format!("{}; прямая сделка Uniswap: {}", a.message, b.message),
        )),
    }
}

/// Котировка умного маршрута из долей: затраты складываются по виду и активу, доли — в маршрут.
/// Ошибки: как у quote/costs.rs.
#[allow(clippy::too_many_arguments)]
fn merge(
    q: &Quoting,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    legs: &[Candidate],
    t: &TokenTaxes,
    gas: U256,
    usdt: &str,
) -> GatewayResult<Quote> {
    let ctx = &q.ctx;
    let coins: Decimal = legs.iter().map(|c| c.coins).sum();
    let paid: Decimal = legs.iter().map(|c| c.usdt).sum();
    let input = |c: &Candidate| if side == Side::Buy { c.usdt } else { c.coins };
    let total_in = if side == Side::Buy { paid } else { coins };
    let mut all: Vec<Cost> = Vec::new();
    let mut route = DexRoute {
        mode: RouteMode::Smart,
        legs: Vec::new(),
        block: u64::MAX,
        gas_units: 0,
    };
    for c in legs {
        let one = costs::quote(ctx, asset, side, c.coins, c, t, gas, usdt)?;
        for cost in one.costs {
            match all.iter_mut().find(|x| {
                x.kind == cost.kind && x.asset == cost.asset && x.in_price == cost.in_price
            }) {
                Some(x) => x.amount += cost.amount,
                None => all.push(cost),
            }
        }
        if let Some(r) = one.route {
            route.block = route.block.min(r.block);
            route.gas_units += r.gas_units;
            for mut leg in r.legs {
                leg.share = input(c) / total_in;
                route.legs.push(leg);
            }
        }
    }
    Ok(Quote {
        venue: ctx.venue(),
        asset: asset.clone(),
        side,
        qty,
        fillable_qty: coins,
        avg_price: paid / coins,
        costs: all,
        route: Some(route),
        at: Utc::now(),
    })
}

/// Выбор большего итога. Ошибок нет (нет цены газа в USDT — сравнение без газа).
async fn pick(q: &Quoting, a: Quote, b: Quote, side: Side, usdt_dec: u8) -> Quote {
    if a.fillable_qty != b.fillable_qty {
        return if a.fillable_qty > b.fillable_qty {
            a
        } else {
            b
        };
    }
    let (ga, gb) = (
        gas_usdt(q, &a, usdt_dec).await,
        gas_usdt(q, &b, usdt_dec).await,
    );
    let (ga, gb) = match (ga, gb) {
        (Some(x), Some(y)) => (x, y),
        _ => (Decimal::ZERO, Decimal::ZERO),
    };
    let (va, vb) = (a.avg_price * a.fillable_qty, b.avg_price * b.fillable_qty);
    let a_better = match side {
        Side::Sell => va - ga >= vb - gb,
        Side::Buy => va + ga <= vb + gb,
    };
    if a_better { a } else { b }
}

/// Затраты котировки в BNB (не в цене), переведённые в USDT по пулу монеты «BNB» из реестра. None — нет пула.
async fn gas_usdt(q: &Quoting, x: &Quote, usdt_dec: u8) -> Option<Decimal> {
    let ctx = &q.ctx;
    let bnb: Decimal = x
        .costs
        .iter()
        .filter(|c| c.asset == BNB && !c.in_price)
        .map(|c| c.amount)
        .sum();
    for e in q.reg.by_ticker(BNB).into_iter().filter(|e| e.valid) {
        let Ok(dec) = q.tokens.decimals(ctx, e.coin).await else {
            continue;
        };
        let Some(units) = num::to_units(bnb, dec) else {
            continue;
        };
        if let Ok(s) = pool_quote::swap(ctx, &e, e.coin, true, units) {
            return num::to_decimal(s.amount_out, usdt_dec);
        }
    }
    None
}
