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
//! Графовый маршрут (RouteMode::Graph, решение 83): перед формированием ордера — предторговая верификация точечным
//! Multicall3 (exec/verify_gate.rs) за правилом шлюза: Reject → ошибка, ордер не формируем; Ok → свежие amountOutMin
//! по ногам ставятся последнему шагу каждой доли.
//! Кто вызывает: mod.rs (контракт trade), exec/trade_smart.rs (общие шаги).
//! От чего зависит: exec/plan.rs, exec/chain.rs, exec/live.rs, exec/order.rs, exec/precheck.rs, exec/precheck_sim.rs (кто меняет), exec/trade_smart.rs, exec/verify_gate.rs, exec/txs.rs, exec/result.rs, quote/direct.rs,
//! wallet/nonce.rs, wallet/inflows.rs, wallet/balances.rs, pools/registry.rs.
//! Настройки: exec.dex_slippage_pct [bsc_dex] (допуск к ожидаемому выходу); через вызываемые — dex.*, send.*, channel.*.
//!
//! Минимум выхода и проверка цены — exec/order.rs (худшая цена ≤ 0 или минимум 0 — ошибка до отправки).
//! Нативная монета BNB (решение 62) — exec/native.rs: WRAP_ETH / UNWRAP_WETH в той же транзакции, резерв на газ не трогается.

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{RouteMode, Side, TradeRequest, TradeResult, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::events::MarketEvent;

use super::super::pools::registry::Entry;
use super::super::quote::via::Via;
use super::super::quote::{Quoting, direct};
use super::super::wallet::balances;
use super::super::wallet::inflows::SwapGuard;
use super::super::wallet::nonce::{Nonces, Slot};
use super::native::{self, Wrap};
pub use super::order::{check_price, order, worst_positive};
use super::plan::{self, Plan};
use super::trade_smart;
use super::txs::{self, Env};
use super::{chain, live, precheck, precheck_sim, result, verify_gate};

/// Сделка. Ошибки: NoRoute, QuoteMismatch (цена хуже допустимой), CoinNotChecked, настройки, каналы,
/// SwapReverted / TxNotIncluded / NonceConflict (с фактом транзакции).
pub async fn trade(
    q: &Quoting,
    env: &Env,
    nonces: &Nonces,
    req: TradeRequest,
) -> GatewayResult<TradeResult> {
    let ctx = &q.ctx;
    // Данные пулов отстают (лента квитанций догоняет, решение 72) — сделки нет.
    q.lag.check(ctx)?;
    let route = req.route.clone().ok_or_else(|| {
        ctx.err(
            ErrorCode::NoRoute,
            "В запросе нет маршрута DEX из котировки",
        )
    })?;
    // Графовый маршрут (решение 80): неисполнимый как есть (мульти-роутер, решение 84) — отказ до сделки (деньги не
    // рискуют); исполнимый — прямой формы (одна доля, один хоп) идёт прямым путём ниже, остальной (всё через один
    // Universal Router PancakeSwap) — умным, тем же кодом, что RouteMode::Smart.
    // Свежие минимумы выхода по ногам из предторговой верификации графового маршрута (решение 83, 80.6b); для
    // одиночной прямой доли графа ставятся ниже, для умной формы — передаются в trade_smart.
    let mut graph_mins: Option<Vec<Decimal>> = None;
    if route.mode == RouteMode::Graph {
        if !route.executable {
            let why = route
                .exec_note
                .clone()
                .unwrap_or_else(|| "граф-маршрут не исполним как есть (решение 84)".to_string());
            return Err(ctx.err(ErrorCode::NoRoute, why));
        }
        // Предторговая верификация за правилом шлюза (решение 83): Reject → ошибка, ордер не формируем;
        // Ok → свежие amountOutMin по ногам.
        let verified = verify_gate::gate(q, env, &req, &route).await?;
        if !(route.legs.len() == 1 && route.legs[0].hops.len() == 1) {
            return trade_smart::trade(q, env, nonces, req, &route, Some(verified.amount_out_min))
                .await;
        }
        graph_mins = Some(verified.amount_out_min);
    }
    let leg = match (route.mode, route.legs.as_slice()) {
        (RouteMode::Smart, _) => {
            return trade_smart::trade(q, env, nonces, req, &route, None).await;
        }
        (RouteMode::Direct | RouteMode::Graph, [leg]) => leg.clone(),
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
    let mut legs = vec![chain::steps(ctx, &req, &path, req.qty, decs)?];
    // Графовый маршрут (решение 83): свежий минимум выхода доли из верификации — последнему шагу (он отдаёт выход).
    if let Some(mins) = &graph_mins {
        let out_dec = if req.side == Side::Buy {
            decs.0
        } else {
            decs.1
        };
        let min = *mins
            .first()
            .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "верификация не вернула минимум выхода"))?;
        verify_gate::apply_leg_min(ctx, &mut legs[0], min, out_dec)?;
    }
    let is_native = native::is_native(ctx, &req.asset);
    let wrap = if is_native {
        native::adapt(ctx, req.side, &mut legs)?
    } else {
        Wrap::None
    };
    for h in &path.hops {
        precheck::quoter_now(ctx, &h.entry, h.token_in, &h.swapped, h.usable.hook_pct).await?;
    }
    let slot = nonces.acquire(ctx).await?;
    let wallet = slot.from;
    let (at, now) = q.heads.at(ctx).await?;
    let p = plan::build(ctx, &legs[0], wallet, now, false)
        .await?
        .with_native(wrap);
    let router = is_native.then_some(p.router);
    let entries: Vec<&Entry> = path.hops.iter().map(|h| &h.entry).collect();
    precheck_sim::emitters(ctx, &p, &entries, at).await?;
    let d = send_plan(env, &slot, p, guard).await?;
    drop(slot);
    let tin = if req.side == Side::Buy { usdt } else { coin };
    result::result(ctx, req, d, wallet, (tin, out_token), decs, router)
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
    let data = p.data(base);
    // Продажа BNB (решение 62): сумма и газ — только сверх резерва на газ (exec/native.rs).
    if !p.value().is_zero() {
        native::reserve_ok(env, p.wallet, (p.router, &data), p.value()).await?;
    }
    txs::send(env, slot, p.router, data, p.value(), true, Some(guard)).await
}
