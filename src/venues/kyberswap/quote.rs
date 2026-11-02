//! Файл: venues/kyberswap/quote.rs
//! Что делает: котировка на объём через агрегатор KyberSwap — перевод GET /routes в тип ядра Quote. Агрегатор сам
//! дробит объём по площадкам, поэтому исполнимый объём = запрошенному, а проскальзывание и комиссии пулов уже внутри
//! amountOut (средней цены, пометка in_price). Газ — отдельной затратой из оценки агрегатора.
//! Кто вызывает: venue.rs (quote), market.rs (цена для «Живого экрана»).
//! От чего зависит: ctx.rs, rest.rs, state.rs, balances.rs (decimals), num.rs, core/types.
//! Настройки: venue.quote_asset [kyberswap] (тикер USDT), kyber.buy_fill_tolerance_pct (допуск недобора при покупке).
//! Решение 95: вызывается и из bsc_dex — финальная котировка расчёта кандидата при exec.via_kyber.
//!
//! Агрегатор отдаёт amountOut для amountIn (входной токен). Продажа монеты (вход — монета) ложится прямо; покупка
//! монеты (вход — USDT, нужен ТОЧНЫЙ выход монет) подбирается: оценка входа обратной котировкой и одно уточнение
//! входа по доле недобора (документация агрегатора — amountIn-маршрутизация, точного выхода у API нет).

use alloy_primitives::{Address, U256};
use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Asset, Cost, CostKind, Quote, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::balances::{BNB, decimals};
use super::ctx::Ctx;
use super::num;
use super::rest::{self, Route};
use super::state::Parts;

/// Доля, ниже которой выход монет при покупке считается недобором (kyber.buy_fill_tolerance_pct: 0,1 → 0,999).
/// Агрегатор маршрутизирует по входу, точного выхода у API нет — недобор в пределах допуска считается полным.
/// Ошибки: NotConfigured.
fn fill_tol(ctx: &Ctx) -> GatewayResult<Decimal> {
    let pct = ctx.dec("kyber.buy_fill_tolerance_pct")?;
    Ok(Decimal::ONE - pct / Decimal::ONE_HUNDRED)
}

/// Адрес USDT площадки: монета с тикером venue.quote_asset [kyberswap] в списке подписки (адрес — из API биржи).
/// Ошибки: NotConfigured — тикер не задан или такой монеты с адресом в подписке нет.
pub fn usdt(p: &Parts) -> GatewayResult<Address> {
    let quote = p.ctx.venue_text("venue.quote_asset")?;
    p.assets()
        .into_iter()
        .find(|a| a.ticker == quote)
        .and_then(|a| a.bsc_contract)
        .ok_or_else(|| {
            p.ctx.not_set(
                "venue.quote_asset",
                format!("В списке монет ядра нет {quote} (venue.quote_asset) с адресом в BSC"),
            )
        })
}

/// Котировка на объём. Принимает: состояние, монету, сторону, количество монеты. Возвращает: Quote.
/// Ошибки: NoRoute (нет адреса монеты / маршрута у агрегатора), NotConfigured, узел (decimals), сеть.
pub async fn quote(p: &Parts, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
    let ctx = &p.ctx;
    let coin = asset.bsc_contract.ok_or_else(|| {
        ctx.err(
            ErrorCode::NoRoute,
            format!("{}: нет адреса контракта в BSC", asset.ticker),
        )
    })?;
    let usdt = usdt(p)?;
    let coin_dec = decimals(p, coin).await?;
    let usdt_dec = decimals(p, usdt).await?;
    let (usdt_amount, coin_amount, route) =
        amounts(p, coin, usdt, (coin_dec, usdt_dec), side, qty).await?;
    let avg_price = if coin_amount.is_zero() {
        return Err(ctx.err(ErrorCode::NoRoute, "агрегатор вернул нулевой объём монеты"));
    } else {
        usdt_amount / coin_amount
    };
    // Исполнимый объём: продажа — весь запрошенный вход; покупка — сколько монет реально берётся (но не больше qty);
    // недобор в пределах kyber.buy_fill_tolerance_pct — полный объём (точного выхода у API агрегатора нет).
    let fillable_qty = match side {
        Side::Sell => qty,
        Side::Buy if coin_amount >= qty * fill_tol(ctx)? => qty,
        Side::Buy => coin_amount.min(qty),
    };
    Ok(Quote {
        venue: ctx.venue(),
        asset: asset.clone(),
        side,
        qty,
        fillable_qty,
        avg_price,
        costs: costs(ctx, &route),
        route: None,
        signal_route: None,
        at: Utc::now(),
    })
}

/// Суммы котировки: (USDT, монета, маршрут для возможной сборки). Продажа — один запрос; покупка — оценка входа и
/// одно уточнение. Ошибки: узел, сеть, NoRoute.
async fn amounts(
    p: &Parts,
    coin: Address,
    usdt: Address,
    (coin_dec, usdt_dec): (u8, u8),
    side: Side,
    qty: Decimal,
) -> GatewayResult<(Decimal, Decimal, Route)> {
    let ctx = &p.ctx;
    match side {
        Side::Sell => {
            let amount_in = units(ctx, qty, coin_dec)?;
            let r = rest::routes(ctx, coin, usdt, amount_in).await?;
            let usdt_out = dec(ctx, r.amount_out, usdt_dec)?;
            Ok((usdt_out, qty, r))
        }
        Side::Buy => buy_route(p, coin, usdt, (coin_dec, usdt_dec), qty).await,
    }
}

/// Маршрут покупки qty монет (вход — USDT): оценка входа обратной котировкой и одно уточнение по недобору.
/// Возвращает: (USDT вход, монеты на выходе, маршрут USDT→монета для сборки). Ошибки: узел, сеть, NoRoute.
pub async fn buy_route(
    p: &Parts,
    coin: Address,
    usdt: Address,
    (coin_dec, usdt_dec): (u8, u8),
    qty: Decimal,
) -> GatewayResult<(Decimal, Decimal, Route)> {
    let ctx = &p.ctx;
    // Оценка входа: столько USDT вернула бы продажа qty монет (чуть меньше нужного — уточняем ниже).
    let sell_in = units(ctx, qty, coin_dec)?;
    let rev = rest::routes(ctx, coin, usdt, sell_in).await?;
    let mut usdt_in = rev.amount_out;
    let mut r = rest::routes(ctx, usdt, coin, usdt_in).await?;
    let mut coins = dec(ctx, r.amount_out, coin_dec)?;
    let tol = fill_tol(ctx)?;
    if !coins.is_zero() && coins < qty * tol {
        // Недобор монет — поднять вход пропорционально (вход_нов = вход · qty / полученные монеты).
        let scaled = dec(ctx, usdt_in, usdt_dec)? * qty / coins;
        usdt_in = units(ctx, scaled, usdt_dec)?;
        r = rest::routes(ctx, usdt, coin, usdt_in).await?;
        coins = dec(ctx, r.amount_out, coin_dec)?;
    }
    Ok((dec(ctx, usdt_in, usdt_dec)?, coins, r))
}

/// Затраты котировки: газ (BNB из gas×gasPrice, иначе USDT из gasUsd). Комиссии пулов и налог свопа уже в средней
/// цене (amountOut агрегатора) — отдельными строками не дублируются. Ошибок нет.
fn costs(ctx: &Ctx, r: &Route) -> Vec<Cost> {
    if let Some(price) = r.gas_price {
        let wei = price.saturating_mul(U256::from(r.gas_units));
        if let Some(bnb) = num::to_decimal(wei, num::BNB_DECIMALS) {
            return vec![Cost::new(CostKind::Gas, bnb, BNB, false)];
        }
    }
    if let Some(usd) = r.gas_usd {
        // Котируемая монета — USDT (venue.quote_asset); газ в USD ≈ USDT.
        if let Ok(quote) = ctx.venue_text("venue.quote_asset") {
            return vec![Cost::new(CostKind::Gas, usd, quote, false)];
        }
    }
    Vec::new()
}

/// Decimal → минимальные единицы. Ошибки: BadResponse.
fn units(ctx: &Ctx, v: Decimal, d: u8) -> GatewayResult<U256> {
    num::to_units(v, d).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("{v} не переводится в единицы токена"),
        )
    })
}

/// Минимальные единицы → Decimal. Ошибки: BadResponse.
fn dec(ctx: &Ctx, v: U256, d: u8) -> GatewayResult<Decimal> {
    num::to_decimal(v, d).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("{v} не помещается в Decimal"),
        )
    })
}
