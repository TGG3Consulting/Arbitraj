//! Файл: venues/bsc_dex/exec/trade.rs
//! Что делает: покупка или продажа на DEX по маршруту из котировки («прямой» режим, один пул): пересчёт по живому
//! состоянию пула, проверка худшей цены, минимум «получить не меньше», разрешения (Permit2), обмен через Universal
//! Router с проверкой реально полученного баланса в той же транзакции, ожидание итога своим узлом, факт по логам.
//! Не исполнилось — ошибка с фактом транзакции (газ мог сгореть), повторов нет (стартовое правило Тиграна).
//! Кто вызывает: mod.rs (контракт trade).
//! От чего зависит: exec/plan.rs, exec/txs.rs, exec/result.rs, quote/direct.rs, wallet/nonce.rs, pools/registry.rs.
//! Настройки: exec.dex_slippage_pct [bsc_dex] (допуск к ожидаемому выходу); через вызываемые — dex.*, send.*, channel.*.
//!
//! Минимум выхода = наибольшее из «по худшей цене» (TradeRequest.worst_price) и «ожидаемое × (1 − допуск)»;
//! «never send amountMin = 0» — руководство PancakeSwap для агентов (research/flow/bsc_dex.md, раздел 5).

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{RouteMode, Side, TradeRequest, TradeResult, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::num;
use super::super::quote::{Quoting, direct};
use super::super::wallet::nonce::Nonces;
use super::plan::{self, Order};
use super::result;
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
        (RouteMode::Direct, [leg]) if leg.hops.len() == 1 => leg.hops[0].clone(),
        _ => {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "Умный маршрут — шаг 1e; сейчас исполняется только прямой обмен в одном пуле",
            ));
        }
    };
    let e = q
        .reg
        .get(&hop.pool)
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "Пул маршрута больше не в реестре"))?;
    let (coin_dec, usdt_dec) = (
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
        coin_dec,
        usdt_dec,
    )?;
    if c.coins.is_zero() {
        return Err(ctx.err(ErrorCode::NoRoute, "Пул сейчас не даёт ни одной монеты"));
    }
    let price = c.usdt / c.coins;
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
    let slip = Decimal::ONE - ctx.dec("exec.dex_slippage_pct")? / Decimal::ONE_HUNDRED;
    let bad = || {
        ctx.err(
            ErrorCode::BadResponse,
            "сумма не переводится в единицы токена",
        )
    };
    let (tin, tout, amount_in, min_out) = match req.side {
        Side::Buy => {
            let need = (c.usdt / req.worst_price).max(c.coins * slip);
            (
                e.usdt,
                e.coin,
                num::to_units(c.usdt, usdt_dec).ok_or_else(bad)?,
                num::to_units(need, coin_dec).ok_or_else(bad)?,
            )
        }
        Side::Sell => {
            let need = (req.qty * req.worst_price).max(c.usdt * slip);
            (
                e.coin,
                e.usdt,
                num::to_units(req.qty, coin_dec).ok_or_else(bad)?,
                num::to_units(need, usdt_dec).ok_or_else(bad)?,
            )
        }
    };
    let slot = nonces.acquire(ctx).await?;
    let wallet = slot.from;
    let now = q.heads.fresh(ctx)?.timestamp;
    let p = plan::build(
        ctx,
        &e,
        wallet,
        &Order {
            token_in: tin,
            token_out: tout,
            amount_in,
            min_out,
        },
        now,
        false,
    )
    .await?;
    for (to, data) in &p.approvals {
        let d = txs::send(env, wallet, *to, data.clone(), U256::ZERO).await?;
        if d.chain.outcome != TxOutcome::Included {
            return Err(ctx
                .err(
                    ErrorCode::SwapReverted,
                    format!("Разрешение (approve) не прошло: {:?}", d.chain.outcome),
                )
                .with_tx(d.chain));
        }
    }
    let d = txs::send(env, wallet, p.router, p.data, U256::ZERO).await?;
    drop(slot);
    result::result(ctx, req, d, wallet, (tin, tout), (coin_dec, usdt_dec))
}
