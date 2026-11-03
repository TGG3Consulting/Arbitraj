//! Файл: venues/kyberswap/trade_prep.rs
//! Что делает: подготовка свопа KyberSwap без отправки (решение 123: «получить маршрут» отдельно от «исполнить»):
//! свежий маршрут на объём сделки (GET /routes; у покупки — подбор входа под точный выход, quote::buy_route), сборка
//! транзакции (POST /route/build со slippageTolerance и deadline), проверка худшей цены (req.worst_price) и вид
//! собранного маршрута в формате котировки ядра — для сравнения со своим маршрутом в режиме «лучший из двух»
//! (bsc_dex/best_of/trade.rs). Тот же ответ /routes и та же сборка идут в своп (trade.rs execute) — лишнего запроса
//! к KyberSwap нет. Ничего не отправляет, nonce не трогает.
//! Кто вызывает: trade.rs (trade, execute — пересборка после approve), bsc_dex/best_of/trade.rs.
//! От чего зависит: ctx.rs, state.rs, rest.rs, quote.rs, balances.rs (decimals), num.rs, signer.rs, core/types.
//! Настройки: kyber.slippage_bps, kyber.deadline_s, kyber.buy_fill_tolerance_pct (через quote.rs).

use alloy_primitives::Address;
use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Quote, Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::balances::decimals;
use super::ctx::Ctx;
use super::rest::{self, Built, Route};
use super::state::Parts;
use super::{num, quote, signer};

/// Подготовленный своп: адреса, decimals, собранная транзакция и её вид котировкой ядра.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub coin: Address,
    pub usdt: Address,
    /// (decimals монеты, decimals USDT).
    pub decs: (u8, u8),
    /// Токен входа и выхода свопа (покупка: USDT → монета; продажа: монета → USDT).
    pub token_in: Address,
    pub token_out: Address,
    pub wallet: Address,
    pub slippage: u64,
    /// Собранная транзакция (POST /route/build) — уже прошла проверку худшей цены.
    pub built: Built,
    /// Вид маршрута котировкой ядра: средняя цена по суммам сборки, исполнимое количество, газ из ответа /routes.
    pub view: Quote,
}

/// Подготовка свопа на объём req.qty. Принимает: состояние, запрос. Возвращает: Prepared.
/// Ошибки: NoRoute (нет адреса / маршрута), QuoteMismatch (цена сборки хуже допустимой), NotConfigured, узел, сеть
/// (RateLimited, Timeout …).
pub async fn prepare(p: &Parts, req: &TradeRequest) -> GatewayResult<Prepared> {
    let ctx = &p.ctx;
    let coin = req.asset.bsc_contract.ok_or_else(|| {
        ctx.err(
            ErrorCode::NoRoute,
            format!("{}: нет адреса контракта в BSC", req.asset.ticker),
        )
    })?;
    let usdt = quote::usdt(p)?;
    let decs = (decimals(p, coin).await?, decimals(p, usdt).await?);
    let (token_in, token_out) = if req.side == Side::Buy {
        (usdt, coin)
    } else {
        (coin, usdt)
    };
    let wallet = signer::address(ctx)?;
    let slippage = ctx.int("kyber.slippage_bps")?;
    let (built, view) = build(p, req, (coin, usdt), decs, wallet, slippage).await?;
    Ok(Prepared {
        coin,
        usdt,
        decs,
        token_in,
        token_out,
        wallet,
        slippage,
        built,
        view,
    })
}

/// Свежий маршрут, сборка и проверка худшей цены. Возвращает: (сборка, её вид котировкой). Вызывается и после approve
/// (решение 99: пересборка). Ошибки: как у prepare.
pub async fn build(
    p: &Parts,
    req: &TradeRequest,
    (coin, usdt): (Address, Address),
    decs: (u8, u8),
    wallet: Address,
    slippage: u64,
) -> GatewayResult<(Built, Quote)> {
    let ctx = &p.ctx;
    let route = fresh_route(p, coin, usdt, decs, req.side, req.qty).await?;
    let deadline = Utc::now().timestamp() as u64 + ctx.int("kyber.deadline_s")?;
    let built = rest::build(ctx, &route, wallet, wallet, slippage, deadline).await?;
    let view = view(ctx, req, &route, &built, decs)?;
    check_price(ctx, req, view.avg_price)?;
    Ok((built, view))
}

/// Свежий маршрут на объём сделки (как котировка). Для покупки вход подбирается под точный выход (quote::buy_route).
/// Ошибки: BadResponse (объём), NoRoute, сеть.
async fn fresh_route(
    p: &Parts,
    coin: Address,
    usdt: Address,
    decs: (u8, u8),
    side: Side,
    qty: Decimal,
) -> GatewayResult<Route> {
    let ctx = &p.ctx;
    match side {
        Side::Sell => {
            let amount_in = num::to_units(qty, decs.0).ok_or_else(|| {
                ctx.err(
                    ErrorCode::BadResponse,
                    "объём не переводится в единицы монеты",
                )
            })?;
            rest::routes(ctx, coin, usdt, amount_in).await
        }
        Side::Buy => {
            // Подбор входа USDT под выход qty монет — тот же, что в котировке.
            let (_usdt, _coins, route) = quote::buy_route(p, coin, usdt, decs, qty).await?;
            Ok(route)
        }
    }
}

/// Суммы сборки: (монеты, USDT) по стороне. Ошибки: BadResponse (не переводятся в числа), NoRoute (монет 0).
fn amounts(
    ctx: &Ctx,
    side: Side,
    b: &Built,
    (cd, ud): (u8, u8),
) -> GatewayResult<(Decimal, Decimal)> {
    let (coins_u, usdt_u) = if side == Side::Buy {
        (b.amount_out, b.amount_in)
    } else {
        (b.amount_in, b.amount_out)
    };
    let (Some(coins), Some(usdt)) = (num::to_decimal(coins_u, cd), num::to_decimal(usdt_u, ud))
    else {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            "суммы сборки не переводятся в числа",
        ));
    };
    if coins.is_zero() {
        return Err(ctx.err(ErrorCode::NoRoute, "сборка вернула нулевой объём монеты"));
    }
    Ok((coins, usdt))
}

/// Проверка худшей цены (req.worst_price) по средней сборки: покупка — не выше, продажа — не ниже.
/// Ошибки: QuoteMismatch.
fn check_price(ctx: &Ctx, req: &TradeRequest, avg: Decimal) -> GatewayResult<()> {
    let bad = match req.side {
        Side::Buy => avg > req.worst_price,
        Side::Sell => avg < req.worst_price,
    };
    if bad {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            format!(
                "средняя цена сборки {avg} хуже допустимой {} ({:?})",
                req.worst_price, req.side
            ),
        ));
    }
    Ok(())
}

/// Вид сборки котировкой ядра (как quote.rs): средняя цена, исполнимое количество (недобор покупки в пределах
/// kyber.buy_fill_tolerance_pct — полный объём), газ из ответа /routes. Ошибки: как у amounts, NotConfigured.
fn view(
    ctx: &Ctx,
    req: &TradeRequest,
    r: &Route,
    b: &Built,
    decs: (u8, u8),
) -> GatewayResult<Quote> {
    let (coins, usdt) = amounts(ctx, req.side, b, decs)?;
    let fillable_qty = match req.side {
        Side::Sell => req.qty,
        Side::Buy if coins >= req.qty * quote::fill_tol(ctx)? => req.qty,
        Side::Buy => coins.min(req.qty),
    };
    Ok(Quote {
        venue: ctx.venue(),
        asset: req.asset.clone(),
        side: req.side,
        qty: req.qty,
        fillable_qty,
        avg_price: usdt / coins,
        costs: quote::costs(ctx, r),
        route: None,
        signal_route: None,
        pick: None,
        at: Utc::now(),
    })
}
