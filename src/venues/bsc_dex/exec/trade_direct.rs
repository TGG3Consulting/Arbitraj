//! Файл: venues/bsc_dex/exec/trade_direct.rs
//! Что делает: сделка прямого маршрута DEX (одна доля: один пул или пара пулов через промежуточную монету, решение 52 —
//! exec/live.rs, exec/chain.rs): пересчёт по живому состоянию пулов, проверка худшей цены, минимум «получить не
//! меньше», сверка с квотером на сумме сделки (решение 32, exec/precheck.rs), разрешения (Permit2), обмен через
//! Universal Router с проверкой реально полученного баланса в той же транзакции, ожидание итога своим узлом, факт по
//! логам. Минимум выхода — exec/leg_min.rs (решение 228; одна доля — как раньше); графовая прямая форма (решение 83) —
//! минимум предторговой верификации нижней границей последнему шагу.
//! Решение 121 B1 (как решение 99): если разрешения отправлялись (каждое ждёт блока — секунды), маршрут, минимумы и
//! calldata обмена собираются ЗАНОВО на живом состоянии и снова сверяются с req.worst_price (у графа — повторная
//! верификация); свежей сборке нужно новое разрешение — своп не отправляется (QuoteMismatch), второго approve нет.
//! Выходной токен не должен ждать поступлений во время обмена (wallet/inflows.rs); чаевые — только к обмену.
//! Итог и ошибку подписывает исполнителем «свой сборщик: прямой» вызывающий (exec/trade.rs, решение 121 B7). Повторов нет (стартовое правило Тиграна).
//! Решение 177: продажа выхода (TradeRequest.exit_position) при расхождении с квотером — продажа по квотеру
//! (exec/exit_quote.rs): ожидаемое, худшая цена и минимумы — от квотера; строка «— продажа по квотеру» — в журнал и в
//! TradeResult.executor (хронология сделки). Покупка — сверка с квотером, как раньше, стоп.
//! Кто вызывает: exec/trade.rs (RouteMode::Direct и графовая прямая форма).
//! От чего зависит: exec/trade.rs (общие шаги), exec/swap_ctx.rs (контекст разбора в «чем найти», решение 231), check/sim_base.rs (блок пробы, решение 127), exec/live.rs, exec/chain.rs, exec/plan.rs, exec/native.rs,
//! exec/precheck.rs, exec/exit_quote.rs, exec/precheck_sim.rs, exec/verify_gate.rs, exec/leg_min.rs, exec/result.rs, quote/direct.rs, quote/via.rs,
//! wallet/nonce.rs, wallet/inflows.rs, pools/registry.rs.
//! Настройки: через вызываемые — exec.dex_slippage_pct, dex.*, verify.*, send.*, channel.*.

use rust_decimal::Decimal;

use crate::core::types::{DexRoute, RouteLeg, Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::events::MarketEvent;

use super::super::check::sim_base::{self, Base};
use super::super::pools::registry::Entry;
use super::super::quote::path::Path;
use super::super::quote::via::Via;
use super::super::quote::{Quoting, direct};
use super::super::wallet::nonce::Nonces;
use super::chain::Step;
use super::native::{self, Wrap};
use super::plan::{self, Plan};
use super::swap_ctx::SwapCtx;
use super::trade::{
    check_price, no_new_approvals, rebuilt_notice, send_approvals, send_swap, sides, worst_positive,
};
use super::txs::Env;
use super::{chain, exit_quote, leg_min, live, precheck_sim, result, verify_gate};

/// Подготовленная доля: путь по живому состоянию, шаги с минимумами, нативная обёртка и пометка «продажа по квотеру»
/// (решение 177; None — по расчёту).
struct Prep {
    path: Path,
    steps: Vec<Step>,
    wrap: Wrap,
    note: Option<String>,
}

/// Сделка прямого маршрута. Принимает: котировку, окружение, очередь кошелька, запрос, маршрут из запроса, его долю,
/// свежие минимумы графовой верификации (Some — графовая прямая форма; None — классика). Ошибки: NoRoute,
/// QuoteMismatch (цена хуже допустимой, новое разрешение после approve), CoinNotChecked, настройки, каналы,
/// SwapReverted / TxNotIncluded / NonceConflict / OutcomeUnknown (с фактом транзакции).
pub async fn trade(
    q: &Quoting,
    env: &Env,
    nonces: &Nonces,
    req: TradeRequest,
    route: &DexRoute,
    leg: &RouteLeg,
    graph_mins: Option<Vec<Decimal>>,
) -> GatewayResult<TradeResult> {
    let ctx = &q.ctx;
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
    let is_native = native::is_native(ctx, &req.asset);
    let mut prep = prepare(&v, &req, leg, graph_mins.as_deref(), decs, is_native).await?;
    let slot = nonces.acquire(ctx).await?;
    let wallet = slot.from;
    let (mut p, mut at) = build(q, &prep, wallet).await?;
    if send_approvals(env, &slot, &p.approvals).await? {
        // Решение 121 B1: разрешения ждали своих блоков — маршрут и минимумы устарели. Графу — свежая верификация
        // (минимумы и граница сделки), затем пересчёт доли по живому состоянию и новая сборка обмена.
        let mins = match graph_mins {
            Some(_) => Some(verify_gate::gate(q, env, &req, route).await?.amount_out_min),
            None => None,
        };
        prep = prepare(&v, &req, leg, mins.as_deref(), decs, is_native).await?;
        (p, at) = build(q, &prep, wallet).await?;
        no_new_approvals(ctx, &p)?;
        rebuilt_notice(env, &req);
    }
    let router = is_native.then_some(p.router);
    let entries: Vec<&Entry> = prep.path.hops.iter().map(|h| &h.entry).collect();
    precheck_sim::emitters(ctx, &p, &entries, at).await?;
    let tin = if req.side == Side::Buy { usdt } else { coin };
    // Решение 231 (дополнение): контекст разбора — в «чем найти» любой ошибки «исход неизвестен» ниже (exec/swap_ctx.rs).
    let sc = SwapCtx::swap(
        wallet,
        (tin, out_token),
        decs,
        router.as_slice(),
        None,
        prep.note.clone(),
    );
    let d = send_swap(env, &slot, p, guard)
        .await
        .map_err(|e| sc.attach(e))?;
    drop(slot);
    let sc = SwapCtx {
        tip_bnb: d.tip_bnb,
        ..sc
    };
    let mut r = result::result(
        ctx,
        req,
        d,
        wallet,
        (tin, out_token),
        decs,
        router.as_slice(),
    )
    .map_err(|e| sc.attach(e))?;
    // Решение 177: «продажа по квотеру» — в хронологию сделки (exec/result.rs by допишет её к исполнителю).
    if prep.note.is_some() {
        r.executor = prep.note;
    }
    Ok(r)
}

/// Доля по живому состоянию: путь (exec/live.rs), проверка худшей цены, шаги с минимумами (exec/chain.rs), свежий
/// минимум графовой верификации последнему шагу, нативная обёртка, сверка каждого хопа с квотером (решение 32).
/// Решение 177: продажа выхода с расхождением — путь пересчитан по квотеру (exec/exit_quote.rs), затем снова проверка
/// худшей цены, шаги, минимумы (свежий минимум графа — тем же множителем) и обёртка; у покупки и без расхождения — как
/// раньше (тот же порядок, сверка — отказ).
/// Принимает: расчёт путей, запрос, долю, минимумы верификации, decimals, нативная ли монета. Ошибки: NoRoute,
/// QuoteMismatch, BadResponse, узел.
async fn prepare(
    v: &Via<'_>,
    req: &TradeRequest,
    leg: &RouteLeg,
    mins: Option<&[Decimal]>,
    decs: (u8, u8),
    is_native: bool,
) -> GatewayResult<Prep> {
    let ctx = &v.q.ctx;
    // Тот же расчёт, что у котировки, но только по пулам маршрута и по состоянию на сейчас (с допуском пулов).
    let mut path = live::path(v, &leg.hops, req.qty, true).await?;
    check_price(ctx, req, path.coins, path.usdt)?;
    let (mut steps, mut wrap) = legs(ctx, req, &path, mins, decs, is_native)?;
    let fixes = exit_quote::check(ctx, &path, exit_quote::exit_of(req), |_| true).await?;
    if fixes.is_empty() {
        return Ok(Prep {
            path,
            steps,
            wrap,
            note: None,
        });
    }
    // Решение 177: ожидаемое — по квотеру; худшая цена и минимумы — заново от него.
    let r = exit_quote::rescale(ctx, &mut path, req.side, &fixes)?;
    check_price(ctx, req, path.coins, path.usdt)?;
    let mins = mins.map(|m| exit_quote::scale_mins(m, r));
    (steps, wrap) = legs(ctx, req, &path, mins.as_deref(), decs, is_native)?;
    let note = exit_quote::report(ctx, req, &fixes).await;
    Ok(Prep {
        path,
        steps,
        wrap,
        note,
    })
}

/// Шаги доли с минимумами (exec/chain.rs), свежий минимум графовой верификации последнему шагу, нативная обёртка.
/// Принимает: окружение, запрос, путь, минимумы верификации, decimals, нативная ли монета. Возвращает: шаги и обёртку.
/// Ошибки: NoRoute, QuoteMismatch, BadResponse, NotConfigured.
fn legs(
    ctx: &super::super::ctx::Ctx,
    req: &TradeRequest,
    path: &Path,
    mins: Option<&[Decimal]>,
    decs: (u8, u8),
    is_native: bool,
) -> GatewayResult<(Vec<Step>, Wrap)> {
    let mut legs = vec![chain::steps(ctx, req, path, req.qty, decs)?];
    // Решение 228 (exec/leg_min.rs): одна доля — наибольшее из границы сделки и ожидаемого × (1 − допуск), как раньше;
    // графовый маршрут (решение 83) — свежий минимум верификации нижней границей (выше ожидаемого — отказ).
    let ver =
        match mins {
            Some(mins) => Some(*mins.first().ok_or_else(|| {
                ctx.err(ErrorCode::NoRoute, "верификация не вернула минимум выхода")
            })?),
            None => None,
        };
    let ver = ver.as_ref().map(std::slice::from_ref);
    leg_min::set_leg_mins(ctx, req, [(path, req.qty)], &mut legs, ver, decs)?;
    let wrap = if is_native {
        native::adapt(ctx, req.side, &mut legs)?
    } else {
        Wrap::None
    };
    Ok((legs.pop().unwrap_or_default(), wrap))
}

/// План обмена на текущем блоке: время блока и разрешения, которых не хватает сейчас (exec/plan.rs). Принимает:
/// котировку, долю, кошелёк. Возвращает: план и базовый блок пробы (голова + 1 с, решение 127 — срок плана от той же
/// головы). Ошибки: NotConfigured, Internal, узел.
async fn build(
    q: &Quoting,
    prep: &Prep,
    wallet: alloy_primitives::Address,
) -> GatewayResult<(Plan, Base)> {
    let (at, now) = sim_base::head(&q.ctx, &q.heads).await?;
    let p = plan::build(&q.ctx, &prep.steps, wallet, now, false)
        .await?
        .with_native(prep.wrap);
    Ok((p, at))
}
