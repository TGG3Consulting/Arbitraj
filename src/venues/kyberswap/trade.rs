//! Файл: venues/kyberswap/trade.rs
//! Что делает: своп через роутер агрегатора KyberSwap: свежий маршрут (GET /routes на объём сделки), проверка худшей
//! цены, сборка транзакции (POST /route/build со slippageTolerance и deadline из настроек), при нехватке разрешения —
//! ERC-20 approve роутеру из ответа, отправка свопа, ожидание квитанции, факт по логам Transfer. Маршрут берётся
//! свежим на сделке (документация: маршрут живёт секунды) — req.route из котировки не используется.
//! Не исполнилось — ошибка с фактом транзакции (газ мог сгореть), повторов нет (стартовое правило Тиграна).
//! Кто вызывает: venue.rs (trade).
//! От чего зависит: ctx.rs, state.rs, rest.rs, tx.rs, node.rs, abi.rs, num.rs, signer.rs, quote.rs, core/types.
//! Настройки: kyber.slippage_bps, kyber.deadline_s; через tx.rs — kyber.gas_*, kyber.receipt_*.

use alloy::sol_types::{SolCall, SolEvent};
use alloy_primitives::{Address, Bytes, U256};
use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Side, TradeRequest, TradeResult, TradeStatus, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::abi::IERC20;
use super::balances::decimals;
use super::ctx::Ctx;
use super::node::{self, Receipt};
use super::rest::{self, Built, Route};
use super::state::Parts;
use super::{num, quote, signer, tx};

/// Своп на объём req.qty. Ошибки: NoRoute, QuoteMismatch (цена хуже допустимой), SwapReverted / TxNotIncluded /
/// OutcomeUnknown (с фактом транзакции), узел, сеть.
pub async fn trade(p: &Parts, req: TradeRequest) -> GatewayResult<TradeResult> {
    let ctx = &p.ctx;
    let coin = req.asset.bsc_contract.ok_or_else(|| {
        ctx.err(
            ErrorCode::NoRoute,
            format!("{}: нет адреса контракта в BSC", req.asset.ticker),
        )
    })?;
    let usdt = quote::usdt(p)?;
    let coin_dec = decimals(p, coin).await?;
    let usdt_dec = decimals(p, usdt).await?;
    // Вход/выход и адрес входного токена для approve.
    let (token_in, token_out) = if req.side == Side::Buy {
        (usdt, coin)
    } else {
        (coin, usdt)
    };
    // Строгая очередь кошелька: approve и своп одной сделки идут без параллельных транзакций (решение 45).
    let _guard = p.wallet_lock.lock().await;
    let wallet = signer::address(ctx)?;

    let route = fresh_route(p, coin, usdt, (coin_dec, usdt_dec), req.side, req.qty).await?;
    let slippage = ctx.int("kyber.slippage_bps")?;
    let deadline = deadline_unix(ctx).await?;
    let mut built = rest::build(ctx, &route, wallet, wallet, slippage, deadline).await?;
    check_price(ctx, &req, &built, (coin_dec, usdt_dec))?;

    // Разрешение на списание входного токена роутером (ERC-20 approve), если его не хватает; у свопа с нативной
    // монетой на входе (value > 0) approve не нужен.
    if built.value.is_zero()
        && ensure_allowance(ctx, token_in, wallet, built.router, built.amount_in).await?
    {
        // Решение 99: approve — отдельная транзакция, ждём её блока (секунды). Маршрут и транзакция свопа, собранные
        // ДО него, к этому моменту устарели — симуляция свопа откатывалась «Return amount is not enough» (сделка
        // 20261101182135312CAKE). Собираем заново на свежем маршруте и снова сверяем худшую цену.
        let route = fresh_route(p, coin, usdt, (coin_dec, usdt_dec), req.side, req.qty).await?;
        let deadline = deadline_unix(ctx).await?;
        built = rest::build(ctx, &route, wallet, wallet, slippage, deadline).await?;
        check_price(ctx, &req, &built, (coin_dec, usdt_dec))?;
        // Новая сборка могла потребовать больше входа (покупка) — второй approve не делаем (снова устареет маршрут).
        if allowance(ctx, token_in, wallet, built.router).await? < built.amount_in {
            return Err(ctx.err(
                ErrorCode::QuoteMismatch,
                "после approve свежий маршрут требует больше входа, чем разрешено — своп не отправлен",
            ));
        }
    }

    let d = tx::send(ctx, wallet, built.router, built.data.clone(), built.value).await?;
    outcome_ok(ctx, &d)?;
    let r = d.receipt.clone().ok_or_else(|| {
        ctx.err(ErrorCode::OutcomeUnknown, "своп в блоке, но нет квитанции")
            .with_tx(d.chain.clone())
    })?;
    result(
        ctx,
        req,
        d,
        &r,
        wallet,
        (token_in, token_out),
        (coin_dec, usdt_dec),
    )
}

/// Свежий маршрут на объём сделки (как котировка). Для покупки вход подбирается под точный выход (quote::amounts).
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

/// Проверка худшей цены (req.worst_price): покупка — средняя не выше, продажа — не ниже. Ошибки: QuoteMismatch.
fn check_price(
    ctx: &Ctx,
    req: &TradeRequest,
    built: &Built,
    (coin_dec, usdt_dec): (u8, u8),
) -> GatewayResult<()> {
    let (coins_u, usdt_u) = if req.side == Side::Buy {
        (built.amount_out, built.amount_in)
    } else {
        (built.amount_in, built.amount_out)
    };
    let coins = num::to_decimal(coins_u, coin_dec);
    let usdt = num::to_decimal(usdt_u, usdt_dec);
    let (Some(coins), Some(usdt)) = (coins, usdt) else {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            "суммы сборки не переводятся в числа",
        ));
    };
    if coins.is_zero() {
        return Err(ctx.err(ErrorCode::NoRoute, "сборка вернула нулевой объём монеты"));
    }
    let avg = usdt / coins;
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

/// Текущее разрешение роутеру на списание токена (ERC-20 allowance). Ошибки: узел.
async fn allowance(
    ctx: &Ctx,
    token: Address,
    wallet: Address,
    router: Address,
) -> GatewayResult<U256> {
    node::view(
        ctx,
        token,
        &IERC20::allowanceCall {
            owner: wallet,
            spender: router,
        },
    )
    .await
}

/// Разрешение на списание токена роутером: читает allowance, при нехватке — approve(router, amount) и ждёт блока.
/// Возвращает: true — approve отправлялся (прошло время, маршрут свопа надо собрать заново).
/// Ошибки: узел, SwapReverted (approve не прошёл — с фактом транзакции), OutcomeUnknown.
async fn ensure_allowance(
    ctx: &Ctx,
    token: Address,
    wallet: Address,
    router: Address,
    amount: U256,
) -> GatewayResult<bool> {
    if allowance(ctx, token, wallet, router).await? >= amount {
        return Ok(false);
    }
    let data = Bytes::from(
        IERC20::approveCall {
            spender: router,
            value: amount,
        }
        .abi_encode(),
    );
    let d = tx::send(ctx, wallet, token, data, U256::ZERO).await?;
    match d.chain.outcome {
        TxOutcome::Included => Ok(true),
        TxOutcome::NotIncluded => Err(ctx
            .err(ErrorCode::TxNotIncluded, "approve не попал в блок")
            .with_tx(d.chain)),
        _ => Err(ctx
            .err(ErrorCode::SwapReverted, "approve не прошёл")
            .with_tx(d.chain)),
    }
}

/// Проверяет исход транзакции свопа. Ошибки: SwapReverted / TxNotIncluded (с фактом транзакции).
fn outcome_ok(ctx: &Ctx, d: &tx::Done) -> GatewayResult<()> {
    match d.chain.outcome {
        TxOutcome::Included => Ok(()),
        TxOutcome::Reverted => Err(ctx
            .err(
                ErrorCode::SwapReverted,
                format!("Своп откатился в блоке (газ сгорел), tx {}", d.chain.hash),
            )
            .with_tx(d.chain.clone())),
        _ => Err(ctx
            .err(
                ErrorCode::TxNotIncluded,
                format!("Своп не попал в блок за срок, tx {}", d.chain.hash),
            )
            .with_tx(d.chain.clone())),
    }
}

/// Факт свопа по квитанции: суммы из логов Transfer кошелька, средняя цена, газ. Ошибки: OutcomeUnknown (суммы не
/// переводятся в числа — с фактом транзакции).
fn result(
    ctx: &Ctx,
    req: TradeRequest,
    d: tx::Done,
    r: &Receipt,
    wallet: Address,
    (tin, tout): (Address, Address),
    (coin_dec, usdt_dec): (u8, u8),
) -> GatewayResult<TradeResult> {
    let spent = moved(r, tin, Some(wallet), None);
    let got = moved(r, tout, None, Some(wallet));
    let (coins_u, usdt_u) = if req.side == Side::Buy {
        (got, spent)
    } else {
        (spent, got)
    };
    let (Some(coins), Some(usdt), Some(fee)) = (
        num::to_decimal(coins_u, coin_dec),
        num::to_decimal(usdt_u, usdt_dec),
        d.chain.gas_bnb,
    ) else {
        return Err(ctx
            .err(
                ErrorCode::OutcomeUnknown,
                format!(
                    "Своп в блоке, tx {}, но суммы не переводятся в числа",
                    d.chain.hash
                ),
            )
            .with_tx(d.chain));
    };
    let status = if coins.is_zero() {
        TradeStatus::NotFilled
    } else if coins < req.qty {
        TradeStatus::PartiallyFilled
    } else {
        TradeStatus::Filled
    };
    Ok(TradeResult {
        status,
        filled_qty: coins,
        avg_price: if coins.is_zero() {
            Decimal::ZERO
        } else {
            usdt / coins
        },
        fee,
        fee_asset: super::balances::BNB.to_string(),
        venue_ref: d.chain.hash.to_string(),
        chain: Some(d.chain),
        finished_at: Utc::now(),
        request: req,
    })
}

/// Сумма переводов токена по квитанции (события Transfer с нужными from/to). Ошибок нет.
pub fn moved(r: &Receipt, token: Address, from: Option<Address>, to: Option<Address>) -> U256 {
    r.logs
        .iter()
        .filter(|l| l.address() == token)
        .filter_map(|l| IERC20::Transfer::decode_log_data(&l.inner.data).ok())
        .filter(|t| from.is_none_or(|f| t.from == f) && to.is_none_or(|x| t.to == x))
        .fold(U256::ZERO, |acc, t| acc.saturating_add(t.value))
}

/// deadline сборки = текущее время блока узла + kyber.deadline_s (Unix-секунды). Ошибки: узел, NotConfigured.
async fn deadline_unix(ctx: &Ctx) -> GatewayResult<u64> {
    let extra = ctx.int("kyber.deadline_s")?;
    Ok(Utc::now().timestamp() as u64 + extra)
}
