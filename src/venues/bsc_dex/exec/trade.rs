//! Файл: venues/bsc_dex/exec/trade.rs
//! Что делает: сделка DEX по маршруту из котировки — выбор пути исполнения и общие шаги. Прямой режим (один пул или
//! пара пулов через промежуточную монету, решение 52) — exec/trade_direct.rs; умный маршрут (RouteMode::Smart) —
//! exec/trade_smart.rs. Общие шаги здесь: адреса и decimals сторон, проверка цены и минимум доли (exec/order.rs),
//! отправка разрешений и обмена — exec/swap_send.rs (реэкспорт отсюда).
//! Решение 121 B1 (как решение 99 у KyberSwap): если разрешения отправлялись — маршрут, минимумы и calldata обмена
//! собираются заново и снова сверяются с req.worst_price; новое разрешение после этого — отказ (no_new_approvals).
//! Не исполнилось — ошибка с фактом транзакции (газ мог сгореть), повторов нет (стартовое правило Тиграна).
//! Графовый маршрут (RouteMode::Graph, решения 83, 84): перед формированием ордера — предторговая верификация точечным
//! Multicall3 (exec/verify_gate.rs) за правилом шлюза: Reject → ошибка, ордер не формируем; Ok → свежие amountOutMin
//! по ногам. Дальше по форме (quote/graph_route::graph_form): прямая (одна доля, один хоп) — exec/trade_direct.rs;
//! один Universal Router PancakeSwap — умным (exec/trade_smart.rs); мульти-роутер — атомарным бандлом (exec/bundle_trade.rs),
//! а при заданном exec.executor_address — одной транзакцией через свой исполнитель ArbExecutor (exec/executor/trade.rs,
//! решение 122, часть X).
//! Кто вызывает: venue.rs (контракт trade); общие шаги — exec/trade_direct.rs, exec/trade_smart.rs,
//! exec/bundle_trade.rs, exec/verify_gate.rs.
//! От чего зависит: exec/trade_direct.rs, exec/trade_smart.rs, exec/bundle_trade.rs, exec/executor/*, exec/verify_gate.rs, exec/order.rs,
//! exec/plan.rs, exec/txs.rs, exec/native.rs, quote/graph_route.rs, wallet/nonce.rs, wallet/inflows.rs, wallet/balances.rs.
//! Настройки: exec.dex_slippage_pct, exec.executor_address [bsc_dex] (допуск; исполнитель); через вызываемые — dex.*, send.*, channel.*.
//! Уведомление: dex.rebuilt_after_approve (маршрут собран заново после разрешений, решение 121 B1).
//! Решение 121 B7: итог и любая ошибка сделки подписаны исполнителем (exec/result.rs by) — здесь, для всех путей
//! («свой сборщик: прямой / умный / бандл / исполнитель»).
//!
//! Минимум выхода и проверка цены — exec/order.rs (худшая цена ≤ 0 или минимум 0 — ошибка до отправки).
//! Нативная монета BNB (решение 62) — exec/native.rs: WRAP_ETH / UNWRAP_WETH в той же транзакции, резерв на газ не трогается.

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{DexRoute, RouteMode, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::quote::Quoting;
use super::super::quote::graph_route::{self, GraphForm};
use super::super::wallet::nonce::Nonces;
pub use super::order::{check_price, order, worst_positive};
pub use super::swap_send::{no_new_approvals, rebuilt_notice, send_approvals, send_swap};
use super::txs::Env;
use super::{bundle_trade, executor, result, trade_direct, trade_smart, verify_gate};

/// Сделка. Итог и любая ошибка подписаны исполнителем (решение 121 B7): «свой сборщик: прямой / умный / бандл» —
/// по форме маршрута из запроса (как выберет run). Ошибки: NoRoute, QuoteMismatch (цена хуже допустимой),
/// CoinNotChecked, настройки, каналы, SwapReverted / TxNotIncluded / NonceConflict (с фактом транзакции).
pub async fn trade(
    q: &Quoting,
    env: &Env,
    nonces: &Nonces,
    req: TradeRequest,
) -> GatewayResult<TradeResult> {
    let who = req.route.as_ref().map_or(result::BY_DIRECT, |r| {
        by_whom(&q.ctx, r, super::native::is_native(&q.ctx, &req.asset))
    });
    result::by(run(q, env, nonces, req).await, who)
}

/// Кто исполнит маршрут — по тем же правилам, что выбор пути в run: умный режим — умный; граф — по форме
/// (quote/graph_route::graph_form), форма «бандл», которую берёт исполнитель (executor::takes), — исполнитель (решение 122);
/// иначе прямой. Ошибок нет (адрес не читается — подпись бандла, сама сделка вернёт ошибку настройки).
fn by_whom(ctx: &Ctx, route: &DexRoute, native: bool) -> &'static str {
    match route.mode {
        RouteMode::Smart => result::BY_SMART,
        RouteMode::Graph => match graph_route::graph_form(ctx, route) {
            GraphForm::Direct => result::BY_DIRECT,
            GraphForm::OneUr => result::BY_SMART,
            GraphForm::Bundle if matches!(executor::takes(ctx, route, native), Ok(Some(_))) => {
                result::BY_EXECUTOR
            }
            GraphForm::Bundle => result::BY_BUNDLE,
        },
        RouteMode::Direct => result::BY_DIRECT,
    }
}

/// Выбор пути исполнения и сама сделка (без подписи исполнителя). Ошибки: как у trade.
async fn run(
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
    // Графовый маршрут (решения 80, 84): неисполнимый как есть — отказ до сделки (деньги не рискуют). Исполнимый —
    // по форме (quote/graph_route::graph_form): прямая (одна доля, один хоп) идёт прямым путём (trade_direct); один Universal
    // Router PancakeSwap — умным путём (как RouteMode::Smart); мульти-роутер — атомарным бандлом свопов (exec/bundle_trade).
    // Предторговая верификация (решение 83) — для всех форм; её свежие amountOutMin по ногам идут в исполнение.
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
        match graph_route::graph_form(ctx, &route) {
            GraphForm::Direct => graph_mins = Some(verified.amount_out_min),
            GraphForm::OneUr => {
                return trade_smart::trade(
                    q,
                    env,
                    nonces,
                    req,
                    &route,
                    Some(verified.amount_out_min),
                )
                .await;
            }
            // Решение 122 (часть X): задан exec.executor_address — одной транзакцией через свой исполнитель.
            GraphForm::Bundle => {
                let is_native = super::native::is_native(ctx, &req.asset);
                if let Some(x) = executor::takes(ctx, &route, is_native)? {
                    return executor::trade::trade(q, env, nonces, req, &route, verified, x).await;
                }
                return bundle_trade::trade(q, env, nonces, req, &route, verified).await;
            }
        }
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
    trade_direct::trade(q, env, nonces, req, &route, &leg, graph_mins).await
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
