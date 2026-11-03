//! Файл: venues/bsc_dex/check/probe_exec.rs
//! Что делает: проба налога (check/probe.rs) через пул вида без своего пути исполнения (family.tradable = false: THENA
//! Solidly / Fusion, Topaz Slipstream, форки V3, DODO, StableSwap, Maverick), исполнимого шагом своего контракта-
//! исполнителя ArbExecutor (pools/exec_kind.rs::via_executor; решение 122, часть X). Своп пробы — тот же вызов, что у
//! настоящей сделки через исполнитель (exec/executor/trade.rs): `execute` развёрнутого исполнителя
//! (exec.executor_address; код и владелец сверены с узлом — exec/executor/onchain.rs) с одним шагом этого пула
//! (exec/executor/call.rs, вид шага — exec/executor/entry.rs) и обычное ERC-20 approve кошелька исполнителю
//! (wallet/approve_executor.rs) — в той же симуляции eth_simulateV1 от кошелька, код не подставляется.
//! Здесь же: причина «пробу собрать нечем» (вида шага нет — не настройка; шаг есть, адрес исполнителя не задан —
//! настройка exec.executor_address) и проверка «кто меняет» для обмена через исполнитель.
//! Кто вызывает: check/probe.rs (part, emitter).
//! От чего зависит: exec/executor/{mod, call, entry, onchain}.rs, exec/chain.rs (Step), exec/plan.rs (Order),
//! wallet/approve_executor.rs, pools/exec_kind.rs, check/emitter.rs, check/simulate.rs (SimLog).
//! Настройки: exec.executor_address [bsc_dex]; через вызываемые — dex.deadline_s, dex.approve_unlimited,
//! dex.v2_fee_pct [DEX].
//!
//! Как налог через исполнитель сопоставлен с настоящей сделкой — docs/notes/probe-executor-2026-11-03.md.
//! Кратко: покупка — выход шага получает кошелёк (у видов со своим выходом — прямо от пула, у StableSwap — через
//! исполнитель и его _sweep), «пришло» = прирост баланса кошелька, т. е. ровно то, что получит кошелёк в сделке;
//! продажа — налог «кошелёк → пул» прямым переводом, как раньше: через исполнитель монета с налогом продаётся только
//! первым шагом «вход переводом в пул» (V2 / SOLIDLY / DODO / WOO — transferFrom кошелёк → пул, один перевод;
//! quote/graph_executor.rs::tax), остальные виды с налогом на входе неисполнимы (сигнал), и это решает сама котировка
//! по измеренному налогу.

use alloy_primitives::{Address, Bytes};

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::ctx::Ctx;
use super::super::exec::chain::Step;
use super::super::exec::executor::{self, call, entry, onchain};
use super::super::exec::plan::Order;
use super::super::pools::exec_kind;
use super::super::pools::registry::Entry;
use super::super::wallet::approve::Approval;
use super::super::wallet::approve_executor;
use super::emitter;
use super::simulate::SimLog;

/// Части свопа пробы через исполнитель: (разрешения кошелька исполнителю, адрес исполнителя, данные execute).
/// Принимает: пул (tradable = false), ордер пробы (минимум 1 единица), кошелёк, время блока. Один шаг на весь вход
/// (10000 bps) — как сделка одной долей через этот пул; минимум execute — минимум ордера.
/// Ошибки: NotConfigured — не задан exec.executor_address (у вида есть шаг) или настройки шага (dex.deadline_s,
/// dex.v2_fee_pct); NoRoute — у вида нет шага исполнителя (не настройка) или пул шагом не исполнить (entry.rs);
/// InvalidAddress — по адресу не наш исполнитель или владелец не кошелёк (onchain.rs); узел.
pub async fn part(
    ctx: &Ctx,
    e: &Entry,
    order: Order,
    wallet: Address,
    now: u64,
) -> GatewayResult<(Vec<Approval>, Address, Bytes)> {
    if !exec_kind::via_executor(ctx, e.family) {
        return Err(no_path(ctx, e));
    }
    let Some(x) = executor::address(ctx)? else {
        return Err(no_path(ctx, e));
    };
    // Ревью 122, пункт 1: адрес вписан руками — до разрешений код и владелец сверяются с узлом (успех помнится).
    onchain::ensure(ctx, x, wallet).await?;
    let step = Step {
        entry: e.clone(),
        order,
        from_wallet: true,
        to_wallet: true,
    };
    let c = call::build(ctx, &[vec![step]], x, now)?;
    // Симуляция не знает текущих разрешений — все сразу (как сухая сделка и проба UR: plan::build(…, true)).
    let approvals = approve_executor::all(ctx, c.token_in, x, c.amount_in)?;
    Ok((approvals, x, c.data()))
}

/// Причина «пробу через этот пул собрать нечем» (решение 122 F) — для журнала и экрана «Монеты». Шаг исполнителя у
/// вида есть, но exec.executor_address не задан — настройка (NotConfigured с её именем: проверка повторится при её
/// задании); вида шага нет — не настройка (NoRoute: «не проверено» с причиной, без записи «не задана настройка»).
/// Принимает: окружение, пул. Возвращает: ошибку. Ошибок иных нет.
pub fn no_path(ctx: &Ctx, e: &Entry) -> GatewayError {
    let t = e.family.title;
    match e.family.executor_kind {
        Some(k) => ctx.not_set(
            "exec.executor_address",
            format!(
                "{t}: проба налога через этот пул идёт шагом {} контракта-исполнителя, а exec.executor_address не \
                 задан; налог — по другим пулам монеты",
                k.name()
            ),
        ),
        None => ctx.err(
            ErrorCode::NoRoute,
            format!(
                "{t}: проба налога через этот пул невозможна — ни своего пути исполнения, ни шага исполнителя у \
                 вида нет; налог — по другим пулам монеты"
            ),
        ),
    }
}

/// «Кто меняет» (решение 59) для обмена пробы. Пул со своим путём (Universal Router / роутер форка) — check/emitter.rs
/// как раньше: роутер сам ищет пару по фабрике. Через исполнитель пул вызывается ровно по адресу шага (как в сделке,
/// exec/executor/sim.rs) — подменить его некому; проверяется, что этот пул в обмене сам выпустил событие (у каждого
/// вида оно есть: Swap / Sync, DODOSwap, TokenExchange, TokenSwap, PoolSwap, WooSwap). Список топиков emitter.rs
/// (V2 / V3 / V4) здесь не годится: события DODO, StableSwap, Maverick в нём нет, а «обмен от другого пула» у монеты с
/// налогом (её обмен накопленного налога внутри перевода) исключил бы чужой по формату пул ложно.
/// Принимает: пул, события вызова обмена. Возвращает: Ok или ошибку с причиной: NotConfigured — пул со своим путём
/// исключён (как раньше, check/probe.rs); NoRoute — через исполнитель событие пула не найдено (не настройка и не
/// вердикт по монете: «не проверено», пул не исключается).
pub async fn emitter(ctx: &Ctx, e: &Entry, logs: &[SimLog]) -> GatewayResult<()> {
    if e.family.tradable {
        return emitter::check(ctx, &[e], logs)
            .await
            .map_err(|why| ctx.err(ErrorCode::NotConfigured, why));
    }
    let pool = entry::supported(e.family, &e.pool.reference)
        .map_err(|why| ctx.err(ErrorCode::NoRoute, why))?
        .1;
    if logs.iter().any(|l| l.address == pool) {
        return Ok(());
    }
    let why = format!(
        "{}: обмен через исполнитель прошёл, но пул {pool:#x} не выпустил ни одного события — проба не засчитана",
        e.family.title
    );
    Err(ctx.err(ErrorCode::NoRoute, why))
}
