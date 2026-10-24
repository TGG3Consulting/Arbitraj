//! Файл: venues/bsc_dex/exec/trade.rs
//! Что делает: покупка или продажа на DEX по маршруту из котировки («прямой» режим: один пул или пара пулов через
//! промежуточную монету, решение 52 — exec/live.rs, exec/chain.rs): пересчёт по живому состоянию пулов, проверка худшей цены, минимум «получить не меньше», разрешения (Permit2), обмен через Universal
//! Router с проверкой реально полученного баланса в той же транзакции, ожидание итога своим узлом, факт по логам.
//! Перед сделкой (решение 32) — повторная проба крючка B/C и сверка с квотером на сумме сделки (exec/precheck.rs);
//! выходной токен не должен ждать поступлений в кошелёк во время обмена (wallet/inflows.rs); база проверки баланса
//! читается прямо перед подписью обмена (после разрешений); чаевые — один раз, к транзакции обмена.
//! Не исполнилось — ошибка с фактом транзакции (газ мог сгореть), повторов нет (стартовое правило Тиграна).
//! Умный маршрут (RouteMode::Smart) — exec/trade_smart.rs; общие шаги (минимум доли, проверка цены, отправка
//! разрешений и обмена) — здесь.
//! Кто вызывает: mod.rs (контракт trade), exec/trade_smart.rs (общие шаги).
//! От чего зависит: exec/plan.rs, exec/chain.rs, exec/live.rs, exec/order.rs, exec/precheck.rs, exec/precheck_sim.rs (кто меняет), exec/trade_smart.rs, exec/txs.rs, exec/result.rs, quote/direct.rs,
//! wallet/nonce.rs, wallet/inflows.rs, wallet/balances.rs, pools/registry.rs.
//! Настройки: exec.dex_slippage_pct [bsc_dex] (допуск к ожидаемому выходу); через вызываемые — dex.*, send.*, channel.*.
//!
//! Минимум выхода и проверка цены — exec/order.rs (худшая цена ≤ 0 или минимум 0 — ошибка до отправки).

use alloy_primitives::{Address, U256};

use crate::core::types::{RouteMode, Side, TradeRequest, TradeResult, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::events::MarketEvent;

use super::super::pools::registry::Entry;
use super::super::quote::via::Via;
use super::super::quote::{Quoting, direct};
use super::super::wallet::balances;
use super::super::wallet::inflows::SwapGuard;
use super::super::wallet::nonce::{Nonces, Slot};
pub use super::order::{check_price, order, worst_positive};
use super::plan::{self, Plan};
use super::trade_smart;
use super::txs::{self, Env};
use super::{chain, live, precheck, precheck_sim, result};

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
    let leg = match (route.mode, route.legs.as_slice()) {
        (RouteMode::Smart, _) => return trade_smart::trade(q, env, nonces, req, &route).await,
        (RouteMode::Direct, [leg]) => leg.clone(),
        _ => {
            return Err(ctx.err(ErrorCode::NoRoute, "Прямой маршрут должен быть одной долей"));
        }
    };
    worst_positive(ctx, &req)?;
    let (coin, usdt, decs) = sides(q, &req).await?;
    let out_token = if req.side == Side::Buy { coin } else { usdt };
    let notice = |text: String| {
        env.stats.emit(MarketEvent::Notice {
            venue: ctx.venue(),
            code: "dex.swap_waits_inflow".into(),
            text,
        });
    };
    let guard = env.inflows.swap(ctx, out_token, &notice).await?;
    let taxes = direct::taxes(q, &req.asset)?;
    let v = Via {
        q,
        coin,
        usdt,
        side: req.side,
        taxes: &taxes,
        coin_dec: decs.0,
        usdt_dec: decs.1,
    };
    // Тот же расчёт, что у котировки, но только по пулам маршрута и по состоянию на сейчас (с допуском пулов).
    let path = live::path(&v, &leg.hops, req.qty, true).await?;
    check_price(ctx, &req, path.coins, path.usdt)?;
    let steps = chain::steps(ctx, &req, &path, req.qty, decs)?;
    for h in &path.hops {
        precheck::quoter_now(ctx, &h.entry, h.token_in, &h.swapped, h.usable.hook_pct).await?;
    }
    let slot = nonces.acquire(ctx).await?;
    let wallet = slot.from;
    let now = q.heads.timestamp(ctx).await?;
    let p = plan::build(ctx, &steps, wallet, now, false).await?;
    let entries: Vec<&Entry> = path.hops.iter().map(|h| &h.entry).collect();
    precheck_sim::emitters(ctx, &p, &entries).await?;
    let d = send_plan(env, &slot, p, guard).await?;
    drop(slot);
    let tin = if req.side == Side::Buy { usdt } else { coin };
    result::result(ctx, req, d, wallet, (tin, out_token), decs)
}

/// Адреса монеты и USDT и их decimals. Ошибки: NoRoute (у монеты нет адреса в BSC), NotConfigured, узел.
pub async fn sides(q: &Quoting, req: &TradeRequest) -> GatewayResult<(Address, Address, (u8, u8))> {
    let ctx = &q.ctx;
    let coin = req.asset.bsc_contract.ok_or_else(|| {
        ctx.err(
            ErrorCode::NoRoute,
            format!("{}: нет адреса контракта в BSC", req.asset.ticker),
        )
    })?;
    let usdt = q.tokens.usdt(ctx)?;
    let decs = (
        q.tokens.decimals(ctx, coin).await?,
        q.tokens.decimals(ctx, usdt).await?,
    );
    Ok((coin, usdt, decs))
}

/// Разрешения по порядку (без чаевых), затем обмен: база проверки баланса — баланс выходного токена кошелька,
/// прочитанный прямо перед подписью обмена; чаевые — к обмену. Принимает: окружение, место в очереди, план,
/// отметку «идёт обмен» (держится до занятия nonce обмена). Возвращает: итог транзакции обмена.
/// Ошибки: каналы, узел (баланс), SwapReverted (разрешение не прошло — с фактом транзакции).
pub async fn send_plan(
    env: &Env,
    slot: &Slot,
    p: Plan,
    guard: SwapGuard,
) -> GatewayResult<txs::Done> {
    for (to, data) in &p.approvals {
        let d = txs::send(env, slot, *to, data.clone(), U256::ZERO, false, None).await?;
        if d.chain.outcome == TxOutcome::Replaced {
            return Err(env
                .ctx
                .err(
                    ErrorCode::OutcomeUnknown,
                    "Исход разрешения (approve) неизвестен — nonce занят, квитанции нет",
                )
                .with_tx(d.chain));
        }
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
    let base = balances::token_units(&env.ctx, p.token_out, p.wallet).await?;
    txs::send(
        env,
        slot,
        p.router,
        p.data(base),
        U256::ZERO,
        true,
        Some(guard),
    )
    .await
}
