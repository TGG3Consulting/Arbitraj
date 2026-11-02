//! Файл: venues/bsc_dex/exec/trade_smart.rs
//! Что делает: сделка по умному маршруту из котировки (решение 38): пересчёт каждой доли по живому состоянию её пулов
//! (один пул или пара через промежуточную монету, решение 52 — exec/live.rs; перед этим — повторная проба крючка B/C,
//! решение 32), проверка средней цены против худшей допустимой, шаги и минимумы каждой доли (exec/chain.rs),
//! ожидаемые поступления выходного токена (wallet/inflows.rs), сверка долей с MixedQuoter PancakeSwap, разрешения
//! и один execute Universal Router PancakeSwap со всеми долями и итоговой проверкой баланса (exec/universal.rs),
//! ожидание итога своим узлом, факт по логам Transfer. Повторов нет (стартовое правило Тиграна).
//! Решение 121 B1 (как решение 99): если разрешения отправлялись (каждое ждёт блока), доли, минимумы и calldata
//! собираются ЗАНОВО на живом состоянии и снова сверяются с req.worst_price и MixedQuoter (у графа — повторная
//! верификация); свежей сборке нужно новое разрешение — своп не отправляется (QuoteMismatch).
//! Итог и ошибку подписывает исполнителем «свой сборщик: умный» вызывающий (exec/trade.rs, решение 121 B7).
//! Кто вызывает: exec/trade.rs.
//! Графовый маршрут (решение 83): вызывающий (exec/trade.rs) передаёт свежие amountOutMin по ногам из предторговой
//! верификации (exec/verify_gate.rs) — они ставятся последнему шагу каждой доли; умный путь без графа передаёт None.
//! От чего зависит: exec/live.rs, exec/chain.rs, exec/universal.rs, exec/precheck_sim.rs (кто меняет), exec/trade.rs (общие шаги), exec/verify_gate.rs, exec/result.rs,
//! quote/direct.rs, quote/via.rs, quote/mixed_quoter.rs, wallet/nonce.rs.
//! Настройки: через вызываемые — smart.*, exec.dex_slippage_pct, dex.*, verify.*, send.*, channel.*.
//!
//! Количество доли: продажа — qty × доля входа (RouteLeg.share); покупка — qty × ожидаемые монеты доли / все
//! ожидаемые монеты; последняя доля получает остаток, чтобы сумма долей была ровно qty.
//! Нативная монета BNB (решение 62) — exec/native.rs: одна WRAP_ETH на все доли продажи / одна UNWRAP_WETH покупки.

use rust_decimal::Decimal;

use crate::core::types::{DexRoute, Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::events::MarketEvent;

use super::super::pools::registry::Entry;
use super::super::quote::mixed_quoter::{self, Leg};
use super::super::quote::path::Path;
use super::super::quote::via::Via;
use super::super::quote::{Quoting, direct};
use super::super::wallet::nonce::Nonces;
use super::chain::Step;
use super::native::{self, Wrap};
use super::plan::Plan;
use super::trade::{
    check_price, no_new_approvals, rebuilt_notice, send_approvals, send_swap, sides, worst_positive,
};
use super::txs::Env;
use super::{chain, live, precheck_sim, result, universal, verify_gate};

/// Подготовленные доли: пути по живому состоянию (с количеством доли), шаги с минимумами, нативная обёртка.
struct Prep {
    paths: Vec<(Path, Decimal)>,
    steps: Vec<Vec<Step>>,
    wrap: Wrap,
}

/// Сделка по умному маршруту. verify_mins — свежие amountOutMin по ногам графовой верификации (решение 83): Some —
/// ставятся последнему шагу каждой доли; None — умный путь без графа, минимумы считает exec/chain.rs. Ошибки: NoRoute
/// (доля не годится, пул пропал или не допущен), QuoteMismatch (цена хуже допустимой, расхождение с MixedQuoter,
/// число ног верификации не совпало, новое разрешение после approve), CoinNotChecked, настройки, каналы,
/// SwapReverted / TxNotIncluded / NonceConflict.
pub async fn trade(
    q: &Quoting,
    env: &Env,
    nonces: &Nonces,
    req: TradeRequest,
    route: &DexRoute,
    verify_mins: Option<Vec<Decimal>>,
) -> GatewayResult<TradeResult> {
    let ctx = &q.ctx;
    worst_positive(ctx, &req)?;
    let taxes = direct::taxes(q, &req.asset)?;
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
    let v = Via {
        q,
        coin,
        usdt,
        side: req.side,
        taxes: &taxes,
        coin_dec: decs.0,
        usdt_dec: decs.1,
    };
    let is_native = native::is_native(ctx, &req.asset);
    let mut prep = prepare(&v, &req, route, verify_mins.as_deref(), decs, is_native).await?;
    let slot = nonces.acquire(ctx).await?;
    let wallet = slot.from;
    let (mut plan, mut at) = build(q, &prep, wallet).await?;
    if send_approvals(env, &slot, &plan.approvals).await? {
        // Решение 121 B1: разрешения ждали своих блоков — доли и минимумы устарели. Графу — свежая верификация, затем
        // пересчёт долей по живому состоянию, сверка с MixedQuoter и новая сборка execute.
        let mins = match verify_mins {
            Some(_) => Some(verify_gate::gate(q, env, &req, route).await?.amount_out_min),
            None => None,
        };
        prep = prepare(&v, &req, route, mins.as_deref(), decs, is_native).await?;
        (plan, at) = build(q, &prep, wallet).await?;
        no_new_approvals(ctx, &plan)?;
        rebuilt_notice(env, &req);
    }
    let router = is_native.then_some(plan.router);
    let entries: Vec<&Entry> = prep
        .paths
        .iter()
        .flat_map(|(p, _)| p.hops.iter().map(|h| &h.entry))
        .collect();
    precheck_sim::emitters(ctx, &plan, &entries, at).await?;
    let d = send_swap(env, &slot, plan, guard).await?;
    drop(slot);
    let tin = if req.side == Side::Buy { usdt } else { coin };
    result::result(ctx, req, d, wallet, (tin, out_token), decs, router)
}

/// Доли по живому состоянию: количество каждой доли, путь (exec/live.rs), проверка средней цены против худшей, сверка
/// с MixedQuoter, шаги и минимумы (exec/chain.rs), свежие минимумы верификации, нативная обёртка.
/// Принимает: расчёт путей, запрос, маршрут, минимумы верификации, decimals, нативная ли монета.
/// Ошибки: NoRoute (нет долей, доля не годится), QuoteMismatch (цена, MixedQuoter, число ног), узел.
async fn prepare(
    v: &Via<'_>,
    req: &TradeRequest,
    route: &DexRoute,
    mins: Option<&[Decimal]>,
    decs: (u8, u8),
    is_native: bool,
) -> GatewayResult<Prep> {
    let ctx = &v.q.ctx;
    let weights: Vec<Decimal> = route
        .legs
        .iter()
        .map(|l| {
            if req.side == Side::Buy {
                l.expected_out
            } else {
                l.share
            }
        })
        .collect();
    let total: Decimal = weights.iter().sum();
    if route.legs.is_empty() || total <= Decimal::ZERO {
        return Err(ctx.err(ErrorCode::NoRoute, "Умный маршрут без долей"));
    }
    let mut paths: Vec<(Path, Decimal)> = Vec::with_capacity(route.legs.len());
    let (mut coins, mut paid, mut left) = (Decimal::ZERO, Decimal::ZERO, req.qty);
    for (i, (leg, w)) in route.legs.iter().zip(&weights).enumerate() {
        let qty = if i + 1 == route.legs.len() {
            left
        } else {
            (req.qty * w / total).min(left)
        };
        left -= qty;
        let p = live::path(v, &leg.hops, qty, false).await?;
        coins += p.coins;
        paid += p.usdt;
        paths.push((p, qty));
    }
    check_price(ctx, req, coins, paid)?;
    let legs: Vec<Leg<'_>> = paths.iter().map(|(p, _)| Leg { path: p }).collect();
    mixed_quoter::check(ctx, &legs).await?;
    let mut steps = Vec::with_capacity(paths.len());
    for (p, qty) in &paths {
        steps.push(chain::steps(ctx, req, p, *qty, decs)?);
    }
    // Графовый маршрут (решение 83): свежие минимумы выхода по ногам из верификации — последнему шагу каждой доли
    // (он отдаёт выход доли). Умный путь без графа передаёт None и ничего не меняет.
    if let Some(mins) = mins {
        if mins.len() != steps.len() {
            return Err(ctx.err(
                ErrorCode::QuoteMismatch,
                "верификация вернула число ног, не равное маршруту",
            ));
        }
        let out_dec = if req.side == Side::Buy {
            decs.0
        } else {
            decs.1
        };
        for (leg, min) in steps.iter_mut().zip(mins) {
            verify_gate::apply_leg_min(ctx, leg, *min, out_dec)?;
        }
    }
    let wrap = if is_native {
        native::adapt(ctx, req.side, &mut steps)?
    } else {
        Wrap::None
    };
    Ok(Prep { paths, steps, wrap })
}

/// План execute на текущем блоке (exec/universal.rs): разрешения, которых не хватает сейчас. Принимает: котировку,
/// доли, кошелёк. Возвращает: план и номер блока (для «кто меняет»). Ошибки: NotConfigured, Internal, узел.
async fn build(
    q: &Quoting,
    prep: &Prep,
    wallet: alloy_primitives::Address,
) -> GatewayResult<(Plan, u64)> {
    let (at, now) = q.heads.at(&q.ctx).await?;
    let plan = universal::build(&q.ctx, &prep.steps, wallet, now)
        .await?
        .with_native(prep.wrap);
    Ok((plan, at))
}
