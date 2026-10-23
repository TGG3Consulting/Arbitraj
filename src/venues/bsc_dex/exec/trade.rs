//! Файл: venues/bsc_dex/exec/trade.rs
//! Что делает: покупка или продажа на DEX по маршруту из котировки («прямой» режим, один пул): пересчёт по живому
//! состоянию пула, проверка худшей цены, минимум «получить не меньше», разрешения (Permit2), обмен через Universal
//! Router с проверкой реально полученного баланса в той же транзакции, ожидание итога своим узлом, факт по логам.
//! Не исполнилось — ошибка с фактом транзакции (газ мог сгореть), повторов нет (стартовое правило Тиграна).
//! Умный маршрут (RouteMode::Smart) — exec/trade_smart.rs; общие шаги (минимум доли, проверка цены, отправка
//! разрешений и обмена) — здесь.
//! Кто вызывает: mod.rs (контракт trade), exec/trade_smart.rs (общие шаги).
//! От чего зависит: exec/plan.rs, exec/trade_smart.rs, exec/txs.rs, exec/result.rs, quote/direct.rs, wallet/nonce.rs,
//! pools/registry.rs.
//! Настройки: exec.dex_slippage_pct [bsc_dex] (допуск к ожидаемому выходу); через вызываемые — dex.*, send.*, channel.*.
//!
//! Минимум выхода = наибольшее из «по худшей цене» (TradeRequest.worst_price) и «ожидаемое × (1 − допуск)»;
//! «never send amountMin = 0» — руководство PancakeSwap для агентов (research/flow/bsc_dex.md, раздел 5).

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{RouteMode, Side, TradeRequest, TradeResult, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::pools::registry::Entry;
use super::super::quote::{Quoting, direct};
use super::super::wallet::nonce::Nonces;
use super::plan::{self, Order, Plan};
use super::result;
use super::trade_smart;
use super::txs::{self, Env};

/// Сделка. Ошибки: NoRoute, QuoteMismatch (цена хуже допустимой), CoinNotChecked, настройки, каналы,
/// SwapReverted / TxNotIncluded / NonceConflict (с фактом транзакции).
pub async fn trade(
    q: &Quoting,
    env: &Env,
    nonces: &Nonces,
    req: TradeRequest,
) -> GatewayResult<TradeResult> {
    let ctx = &q.ctx;
    let route = req.route.clone().ok_or_else(|| {
        ctx.err(
            ErrorCode::NoRoute,
            "В запросе нет маршрута DEX из котировки",
        )
    })?;
    let hop = match (route.mode, route.legs.as_slice()) {
        (RouteMode::Smart, _) => return trade_smart::trade(q, env, nonces, req, &route).await,
        (RouteMode::Direct, [leg]) if leg.hops.len() == 1 => leg.hops[0].clone(),
        _ => {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "Прямой маршрут должен быть одной долей в одном пуле",
            ));
        }
    };
    let e = q
        .reg
        .get(&hop.pool)
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "Пул маршрута больше не в реестре"))?;
    let decs = (
        q.tokens.decimals(ctx, e.coin).await?,
        q.tokens.decimals(ctx, e.usdt).await?,
    );
    // Тот же расчёт, что у котировки, но только по пулу маршрута и по состоянию на сейчас (с допуском пула).
    let (c, _) = direct::best(
        q,
        &req.asset,
        req.side,
        req.qty,
        vec![e.clone()],
        decs.0,
        decs.1,
    )?;
    check_price(ctx, &req, c.coins, c.usdt)?;
    let o = order(ctx, &req, &e, req.qty, (c.coins, c.usdt), decs)?;
    let slot = nonces.acquire(ctx).await?;
    let wallet = slot.from;
    let now = q.heads.fresh(ctx)?.timestamp;
    let p = plan::build(ctx, &e, wallet, &o, now, false).await?;
    let d = send_plan(env, wallet, p).await?;
    drop(slot);
    result::result(ctx, req, d, wallet, (o.token_in, o.token_out), decs)
}

/// Цена по живому состоянию не хуже допустимой. Принимает: запрос, монет и USDT по расчёту.
/// Ошибки: NoRoute (ни одной монеты), QuoteMismatch (цена хуже TradeRequest.worst_price).
pub fn check_price(
    ctx: &Ctx,
    req: &TradeRequest,
    coins: Decimal,
    usdt: Decimal,
) -> GatewayResult<()> {
    if coins.is_zero() {
        return Err(ctx.err(ErrorCode::NoRoute, "Пул сейчас не даёт ни одной монеты"));
    }
    let price = usdt / coins;
    let worse = match req.side {
        Side::Buy => price > req.worst_price,
        Side::Sell => price < req.worst_price,
    };
    if worse {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            format!(
                "Цена в пуле сейчас {price} хуже допустимой {}",
                req.worst_price
            ),
        ));
    }
    Ok(())
}

/// Обмен одной доли: токены, вход и минимум выхода (см. шапку). Принимает: запрос, пул, монет доли (продажа),
/// (монет, USDT) по расчёту доли, (decimals монеты, USDT). Ошибки: NotConfigured (exec.dex_slippage_pct), BadResponse.
pub fn order(
    ctx: &Ctx,
    req: &TradeRequest,
    e: &Entry,
    qty: Decimal,
    (coins, usdt): (Decimal, Decimal),
    (coin_dec, usdt_dec): (u8, u8),
) -> GatewayResult<Order> {
    let slip = Decimal::ONE - ctx.dec("exec.dex_slippage_pct")? / Decimal::ONE_HUNDRED;
    let bad = || {
        ctx.err(
            ErrorCode::BadResponse,
            "сумма не переводится в единицы токена",
        )
    };
    Ok(match req.side {
        Side::Buy => Order {
            token_in: e.usdt,
            token_out: e.coin,
            amount_in: num::to_units(usdt, usdt_dec).ok_or_else(bad)?,
            min_out: num::to_units((usdt / req.worst_price).max(coins * slip), coin_dec)
                .ok_or_else(bad)?,
        },
        Side::Sell => Order {
            token_in: e.coin,
            token_out: e.usdt,
            amount_in: num::to_units(qty, coin_dec).ok_or_else(bad)?,
            min_out: num::to_units((qty * req.worst_price).max(usdt * slip), usdt_dec)
                .ok_or_else(bad)?,
        },
    })
}

/// Разрешения по порядку, затем обмен. Возвращает: итог транзакции обмена.
/// Ошибки: каналы, SwapReverted (разрешение не прошло — с фактом транзакции).
pub async fn send_plan(env: &Env, wallet: Address, p: Plan) -> GatewayResult<txs::Done> {
    for (to, data) in &p.approvals {
        let d = txs::send(env, wallet, *to, data.clone(), U256::ZERO).await?;
        if d.chain.outcome != TxOutcome::Included {
            return Err(env
                .ctx
                .err(
                    ErrorCode::SwapReverted,
                    format!("Разрешение (approve) не прошло: {:?}", d.chain.outcome),
                )
                .with_tx(d.chain));
        }
    }
    txs::send(env, wallet, p.router, p.data, U256::ZERO).await
}
