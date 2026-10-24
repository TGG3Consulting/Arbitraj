//! Файл: venues/bsc_dex/exec/precheck_sim.rs
//! Что делает: проба сделки прямо перед отправкой (решение 59): разрешения и вызов Universal Router из плана
//! симулируются eth_simulateV1 от имени кошелька, и по событиям обмена проверяется, что меняли именно пулы маршрута
//! (check/emitter.rs) — Universal Router сам находит пары по своей фабрике, адрес проверенного пула ему не передаётся.
//! Чужой пул — пул исключается, сделки нет. Проба откатилась — сделки нет (с текстом узла); откат «срок транзакции
//! вышел» — StaleData (наша помеха, не свойство пула).
//! Кто вызывает: exec/trade.rs, exec/trade_smart.rs.
//! От чего зависит: check/simulate.rs, check/emitter.rs, exec/plan.rs (Plan), wallet/balances.rs, pools/registry.rs.
//! Настройки: нет (узел — bsc.node_http_url).
//!
//! eth_simulateV1 — execution-apis 16caee27 src/eth/execute.yaml#L137-L260 (CallResult.logs).

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::emitter;
use super::super::check::simulate::{self, Call};
use super::super::ctx::Ctx;
use super::super::pools::registry::Entry;
use super::super::wallet::balances;
use super::plan::Plan;
use super::ur;

/// Проба плана. Принимает: план (разрешения + обмен), пулы маршрута, номер блока, по времени которого считан срок. Ошибки: NoRoute (обмен не в пулах маршрута —
/// пулы исключены; проба откатилась), узел.
pub async fn emitters(ctx: &Ctx, p: &Plan, entries: &[&Entry], at: u64) -> GatewayResult<()> {
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
    calls.push(Call {
        from: p.wallet,
        to: p.router,
        input: p.data(base),
    });
    let res = simulate::run(ctx, &calls, at).await?;
    if let Some((i, r)) = res.iter().enumerate().find(|(_, r)| !r.ok()) {
        let text = r
            .error
            .as_ref()
            .map(|e| e.message.as_str())
            .unwrap_or_default();
        // Срок транзакции вышел (TransactionDeadlinePassed): время плана устарело в очереди узла — это наша помеха,
        // не свойство пула или монеты: StaleData (сделки нет, пулы не исключаются, монета не в чёрный список).
        if ur::deadline_passed(&r.return_data, text) {
            let msg = format!(
                "проба сделки перед отправкой: шаг {i} — TransactionDeadlinePassed: срок транзакции меньше шага времени симулированного блока (у BSC +12 с к базовому, check/simulate.rs) — увеличьте dex.deadline_s: {}",
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
