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
//! TradeResult.executor (хронология сделки). Остальные — решение 261: сверка односторонняя (квотер меньше расчёта
//! больше допуска — стоп; больше — сделка идёт, строка «в нашу сторону» в хронологию — exec/quoter_favour.rs).
//! Решение 248 (кейс SMARS 10.11): проба продажи выхода откатилась «мало на выходе» (custom error Universal Router,
//! exec/ur_short.rs) — пере-замер реального выхода и новый минимум (exec/ur_remeasure.rs), как 247 у исполнителя.
//! Кто вызывает: exec/trade.rs (RouteMode::Direct и графовая прямая форма).
//! От чего зависит: exec/trade.rs (общие шаги), exec/swap_ctx.rs (контекст разбора в «чем найти», решение 231), check/sim_base.rs (блок пробы, решение 127), exec/live.rs, exec/chain.rs, exec/plan.rs, exec/native.rs,
//! exec/trade_direct_prep.rs (подготовка доли), exec/quoter_favour.rs (журнал «в нашу сторону»), exec/ur_remeasure.rs (проба и пере-замер), exec/executor/trade.rs (expected, coin_tax), exec/verify_gate.rs, exec/leg_min.rs, exec/result.rs, quote/direct.rs, quote/via.rs,
//! wallet/nonce.rs, wallet/inflows.rs, pools/registry.rs.
//! Настройки: через вызываемые — exec.dex_slippage_pct, dex.*, verify.*, send.*, channel.*.

use rust_decimal::Decimal;

use crate::core::types::{DexRoute, RouteLeg, Side, TradeRequest, TradeResult};
use crate::gateway::errors::GatewayResult;
use crate::gateway::events::MarketEvent;

use super::super::check::sim_base::{self, Base};
use super::super::pools::registry::Entry;
use super::super::quote::via::Via;
use super::super::quote::{Quoting, direct};
use super::super::wallet::nonce::Nonces;
use super::executor::trade::{coin_tax, expected};
use super::native;
use super::plan::{self, Plan};
use super::swap_ctx::SwapCtx;
use super::trade::{
    no_new_approvals, rebuilt_notice, send_approvals, send_swap, sides, with_approve_gas,
    worst_positive,
};
use super::trade_direct_prep::{Prep, prepare};
use super::txs::Env;
use super::{quoter_favour, result, ur_remeasure, verify_gate};

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
    let approved = send_approvals(env, &slot, &p.approvals).await?;
    // Решение 236, дополнение (техлид 09.11): газ разрешений — в итог обмена и в любую ошибку после них (swap_send.rs).
    let out: GatewayResult<TradeResult> = async {
        if approved.is_some() {
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
        // Решение 248 (кейс SMARS): откат продажи выхода «мало на выходе» — пере-замер реального выхода
        // (exec/ur_remeasure.rs): план заново с минимумами шагов 1 ед., замер, новый минимум проверки баланса.
        if let Some(orig) = ur_remeasure::check(env, &req, &p, &entries, at).await? {
            let exp = [(expected(&prep.path, req.side), coin_tax(&v, &prep.path))];
            let old = p.min_total();
            ur_remeasure::unit_mins(prep.steps.iter_mut());
            let (probe, at2) = build(q, &prep, wallet).await?;
            no_new_approvals(ctx, &probe)?;
            let (fixed, line) =
                ur_remeasure::measure(env, &req, probe, &entries, at2, &exp, (old, decs.1), orig)
                    .await?;
            p = fixed;
            prep.note = Some(
                prep.note
                    .take()
                    .map_or(line.clone(), |n| format!("{n}; {line}")),
            );
        }
        // Решение 261: «в нашу сторону» — в журнал один раз, по последней подготовке.
        quoter_favour::log(ctx, &req, &prep.favour).await;
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
    .await;
    with_approve_gas(out, approved)
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
