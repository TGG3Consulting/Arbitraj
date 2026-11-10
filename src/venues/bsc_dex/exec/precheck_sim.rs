//! Файл: venues/bsc_dex/exec/precheck_sim.rs
//! Что делает: проба сделки прямо перед отправкой (решение 59): разрешения и вызов Universal Router из плана
//! симулируются eth_simulateV1 от имени кошелька, и по событиям обмена проверяется, что меняли именно пулы маршрута
//! (check/emitter.rs) — Universal Router сам находит пары по своей фабрике, адрес проверенного пула ему не передаётся.
//! Чужой пул — пул исключается, сделки нет. Проба откатилась — сделки нет (с текстом узла); откат «срок транзакции
//! вышел» — StaleData (наша помеха, не свойство пула).
//! Решение 248: run и judge — по отдельности для пере-замера Universal Router (exec/ur_remeasure.rs).
//! Кто вызывает: exec/trade_direct.rs, exec/trade_smart.rs, exec/ur_remeasure.rs.
//! От чего зависит: check/simulate.rs (Base — голова + 1 с, решение 127), check/emitter.rs, exec/plan.rs (Plan), wallet/balances.rs, pools/registry.rs.
//! Настройки: нет (узел — bsc.node_http_url).
//!
//! eth_simulateV1 — execution-apis 16caee27 src/eth/execute.yaml#L137-L260 (CallResult.logs).

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::emitter;
use super::super::check::simulate::{self, Base, Call, CallResult};
use super::super::ctx::Ctx;
use super::super::pools::registry::Entry;
use super::super::wallet::balances;
use super::plan::Plan;
use super::ur;

/// Проба плана. Принимает: план (разрешения + обмен), пулы маршрута, базовый блок — голова, по времени которой считан
/// срок, симулированный блок на 1 с позже (check/sim_base.rs head, решение 127). Ошибки: NoRoute (обмен не в пулах
/// маршрута — пулы исключены; проба откатилась), StaleData (срок вышел), узел.
pub async fn emitters(ctx: &Ctx, p: &Plan, entries: &[&Entry], at: Base) -> GatewayResult<()> {
    let res = run(ctx, p, at, None).await?;
    judge(ctx, &res, entries).await
}

/// Симуляция плана: разрешения, затем execute (с суммой BNB нативной продажи). Решение 248: `around` — чтение баланса
/// выходного токена кошелька до и после execute в том же блоке (замер реального выхода, exec/ur_remeasure.rs).
/// Принимает: план, базовый блок, вызов чтения баланса (None — без замера). Возвращает: итоги вызовов по порядку
/// (с замером — [разрешения…, баланс до, execute, баланс после]). Ошибки: узел, BadResponse.
pub async fn run(
    ctx: &Ctx,
    p: &Plan,
    at: Base,
    around: Option<Call>,
) -> GatewayResult<Vec<CallResult>> {
    let base = balances::token_units(ctx, p.token_out, p.wallet).await?;
    let mut calls: Vec<Call> = p
        .approvals
        .iter()
        .map(|(to, d)| Call {
            from: p.wallet,
            to: *to,
            input: d.clone(),
        })
        .collect();
    calls.extend(around.clone());
    calls.push(Call {
        from: p.wallet,
        to: p.router,
        input: p.data(base),
    });
    // Нативная продажа (решение 62): BNB уходит с вызовом роутера (WRAP_ETH) — та же сумма и в пробе.
    let n = calls.len() - 1;
    calls.extend(around);
    simulate::run_valued(ctx, &calls, &[(n, p.value())], at).await
}

/// Разбор итогов пробы: первый откат — ошибка с текстом узла (срок — StaleData), иначе проверка «кто меняет» по
/// событиям последнего успешного вызова execute. Принимает: итоги run (без замера), пулы маршрута. Ошибки: как emitters.
pub async fn judge(ctx: &Ctx, res: &[CallResult], entries: &[&Entry]) -> GatewayResult<()> {
    if let Some((i, r)) = res.iter().enumerate().find(|(_, r)| !r.ok()) {
        let text = r
            .error
            .as_ref()
            .map(|e| e.message.as_str())
            .unwrap_or_default();
        // Срок транзакции вышел (TransactionDeadlinePassed): симулированный блок — голова + 1 с (решение 127), значит
        // голова устарела в очереди узла или dex.deadline_s < 1 — наша помеха, не свойство пула или монеты: StaleData
        // (сделки нет, пулы не исключаются, монета не в чёрный список).
        if ur::deadline_passed(&r.return_data, text) {
            let msg = format!(
                "проба сделки перед отправкой: шаг {i} — TransactionDeadlinePassed: срок транзакции вышел к симулированному блоку (время головы + 1 с, check/sim_base.rs) — проверьте dex.deadline_s и свежесть узла: {}",
                r.why()
            );
            return Err(ctx.err(ErrorCode::StaleData, msg));
        }
        let msg = format!(
            "проба сделки перед отправкой: шаг {i} откатился — {}",
            r.why()
        );
        return Err(ctx.err(ErrorCode::NoRoute, msg));
    }
    let logs = res.last().map(|r| r.logs.as_slice()).unwrap_or_default();
    emitter::check(ctx, entries, logs)
        .await
        .map_err(|why| ctx.err(ErrorCode::NoRoute, why))
}
