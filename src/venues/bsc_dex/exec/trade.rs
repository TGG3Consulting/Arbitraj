//! Файл: venues/bsc_dex/exec/trade.rs
//! Что делает: сделка DEX по маршруту из котировки — выбор пути исполнения и общие шаги. Прямой режим (один пул или
//! пара пулов через промежуточную монету, решение 52) — exec/trade_direct.rs; умный маршрут (RouteMode::Smart) —
//! exec/trade_smart.rs. Общие шаги здесь: адреса и decimals сторон, проверка цены и минимум доли (exec/order.rs),
//! отправка разрешений (каждое ждёт блока) и обмена (база проверки баланса — прямо перед подписью, чаевые — к обмену).
//! Решение 121 B1 (как решение 99 у KyberSwap): если разрешения отправлялись — маршрут, минимумы и calldata обмена
//! собираются заново и снова сверяются с req.worst_price; новое разрешение после этого — отказ (no_new_approvals).
//! Не исполнилось — ошибка с фактом транзакции (газ мог сгореть), повторов нет (стартовое правило Тиграна).
//! Графовый маршрут (RouteMode::Graph, решения 83, 84): перед формированием ордера — предторговая верификация точечным
//! Multicall3 (exec/verify_gate.rs) за правилом шлюза: Reject → ошибка, ордер не формируем; Ok → свежие amountOutMin
//! по ногам. Дальше по форме (quote/graph_route::graph_form): прямая (одна доля, один хоп) — exec/trade_direct.rs;
//! один Universal Router PancakeSwap — умным (exec/trade_smart.rs); мульти-роутер — атомарным бандлом (exec/bundle_trade.rs).
//! Кто вызывает: venue.rs (контракт trade); общие шаги — exec/trade_direct.rs, exec/trade_smart.rs,
//! exec/bundle_trade.rs, exec/verify_gate.rs.
//! От чего зависит: exec/trade_direct.rs, exec/trade_smart.rs, exec/bundle_trade.rs, exec/verify_gate.rs, exec/order.rs,
//! exec/plan.rs, exec/txs.rs, exec/native.rs, quote/graph_route.rs, wallet/nonce.rs, wallet/inflows.rs, wallet/balances.rs.
//! Настройки: exec.dex_slippage_pct [bsc_dex] (допуск к ожидаемому выходу); через вызываемые — dex.*, send.*, channel.*.
//! Уведомление: dex.rebuilt_after_approve (маршрут собран заново после разрешений, решение 121 B1).
//!
//! Минимум выхода и проверка цены — exec/order.rs (худшая цена ≤ 0 или минимум 0 — ошибка до отправки).
//! Нативная монета BNB (решение 62) — exec/native.rs: WRAP_ETH / UNWRAP_WETH в той же транзакции, резерв на газ не трогается.

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{RouteMode, TradeRequest, TradeResult, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::events::MarketEvent;

use super::super::ctx::Ctx;
use super::super::quote::Quoting;
use super::super::quote::graph_route::{self, GraphForm};
use super::super::wallet::approve::Approval;
use super::super::wallet::balances;
use super::super::wallet::inflows::SwapGuard;
use super::super::wallet::nonce::{Nonces, Slot};
use super::native;
pub use super::order::{check_price, order, worst_positive};
use super::plan::Plan;
use super::txs::{self, Env};
use super::{bundle_trade, trade_direct, trade_smart, verify_gate};

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
            GraphForm::Bundle => {
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

/// Разрешения по порядку (без чаевых), каждое ждёт своего блока. Принимает: окружение, место в очереди, разрешения
/// плана. Возвращает: true — хоть одно отправлено (прошли секунды блоков: маршрут, минимумы и calldata обмена
/// устарели и собираются заново — решение 121 B1, как решение 99 у KyberSwap); false — разрешений не было.
/// Ошибки: каналы, OutcomeUnknown (nonce занят, квитанции нет), SwapReverted (разрешение не прошло — с фактом).
pub async fn send_approvals(env: &Env, slot: &Slot, approvals: &[Approval]) -> GatewayResult<bool> {
    for (to, data) in approvals {
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
    Ok(!approvals.is_empty())
}

/// Свежая сборка после разрешений (решение 121 B1) не должна требовать новых: иначе вход вырос (покупка) или
/// разрешение не легло — второй approve не делаем (маршрут снова устареет), своп не отправляем. Ошибки: QuoteMismatch.
pub fn no_new_approvals(ctx: &Ctx, p: &Plan) -> GatewayResult<()> {
    if p.approvals.is_empty() {
        return Ok(());
    }
    Err(ctx.err(
        ErrorCode::QuoteMismatch,
        "после approve свежий маршрут требует нового разрешения (вход больше разрешённого) — своп не отправлен",
    ))
}

/// Уведомление «после approve маршрут собран заново» (решение 121 B1) — в журнале интерфейса и в Telegram, если
/// событие dex.rebuilt_after_approve включено. Принимает: окружение, запрос. Ошибок нет.
pub fn rebuilt_notice(env: &Env, req: &TradeRequest) {
    env.stats.emit(MarketEvent::Notice {
        venue: env.ctx.venue(),
        code: "dex.rebuilt_after_approve".into(),
        text: format!(
            "{:?} {} {}: разрешения (approve) отправлены — маршрут, минимумы и транзакция обмена собраны заново и \
             снова сверены с худшей ценой {}",
            req.side, req.qty, req.asset.ticker, req.worst_price
        ),
    });
}

/// Обмен после разрешений: база проверки баланса — баланс выходного токена кошелька, прочитанный прямо перед подписью
/// обмена; чаевые — к обмену. Принимает: окружение, место в очереди, план, отметку «идёт обмен» (держится до занятия
/// nonce обмена). Возвращает: итог транзакции обмена. Ошибки: каналы, узел (баланс), резерв BNB.
pub async fn send_swap(
    env: &Env,
    slot: &Slot,
    p: Plan,
    guard: SwapGuard,
) -> GatewayResult<txs::Done> {
    let base = balances::token_units(&env.ctx, p.token_out, p.wallet).await?;
    let data = p.data(base);
    // Продажа BNB (решение 62): сумма и газ — только сверх резерва на газ (exec/native.rs).
    if !p.value().is_zero() {
        native::reserve_ok(env, p.wallet, (p.router, &data), p.value()).await?;
    }
    txs::send(env, slot, p.router, data, p.value(), true, Some(guard)).await
}
