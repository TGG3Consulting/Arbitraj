//! Файл: venues/bsc_dex/exec/executor/mod.rs
//! Что делает: свой контракт-исполнитель ArbExecutor (решение 122, часть X; контракт — contracts/ArbExecutor.sol):
//! маршрут, которого не исполняет один Universal Router, — одной транзакцией через исполнитель, если задан
//! адрес исполнителя кошелька (решение 132; до него — exec.executor_address); не задан — как раньше (бандл, решение 84).
//! Здесь — адрес исполнителя кошелька и выбор пути.
//! Публичный интерфейс для новых семейств (часть F): kind.rs (Params, StepSpec, encode), call.rs (ExecCall, build).
//! Кто вызывает: exec/trade.rs (выбор пути), exec/dry_build.rs (сухая сделка), quote/approve_gas.rs (разрешение),
//! venue.rs (развёртывание).
//! От чего зависит: exec/executor/*, quote/graph_route.rs (graph_form), quote/graph_exec.rs (bundle_first — ревью t181 Н3),
//! pools/registry.rs (ключ пула менеджера для entry::fits — ревью 2 t181), ctx.rs.
//! Настройки: нет (адрес исполнителя — у кошелька, экран «Маппинг кошельков»).

pub mod abi;
pub mod call;
pub mod callback_at;
pub mod code;
pub mod deploy;
pub mod deploy_gate;
pub mod dry;
pub mod entry;
pub mod fact;
pub mod kind;
pub mod kind_buffer;
pub mod kind_key;
pub mod live_class;
pub mod live_proof;
pub mod live_record;
pub mod live_row;
pub mod live_rule;
pub mod onchain;
pub mod pass_view;
pub mod pool_gate;
pub mod pool_pass;
pub mod prep_refetch;
pub mod remeasure;
pub mod sim;
pub mod trade;
pub mod v1_notice;

use alloy_primitives::Address;

use crate::core::types::{DexRoute, RouteMode};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::pools::registry::Registry;
use super::super::pools::{exec_kind, families};
use super::super::quote::graph_route::{self, GraphForm};

/// Адрес развёрнутого исполнителя кошелька окружения (решение 132: у каждого кошелька свой — ctx_wallet.rs).
/// Принимает: окружение. Возвращает: None — не развёрнут (маршруты через разные роутеры идут бандлом, как раньше).
/// Ошибки: Internal — адрес записан не адресом.
pub fn address(ctx: &Ctx) -> GatewayResult<Option<Address>> {
    ctx.executor()
}

/// Решение 272: идёт ли нативный BNB монетой сделки через исполнитель кошелька окружения — native_block пуст. Ошибок нет.
pub fn native_capable(ctx: &Ctx) -> bool {
    native_block(ctx).is_none()
}

/// Решение 272 (+ ревью А1): почему нативный BNB монетой сделки НЕ идёт через исполнитель кошелька окружения. None —
/// идёт: исполнитель задан, сверен как v2 или v3 (onchain.rs → запись кошелька) и обёртка, записанная в его конструкторе
/// (слот wnative, прочитан при сверке), равна dex.native_wrapped. Иначе — точная причина (v1 / не сверен / обёртка не
/// та / настройка не задана); поведение — как до решения 272 (ERC-20 путь не затронут). Ошибок нет.
pub fn native_block(ctx: &Ctx) -> Option<String> {
    let w = match ctx.wallet() {
        Ok(w) => w,
        Err(e) => return Some(e.message),
    };
    let Some(x) = w.executor.as_deref() else {
        return Some(format!("у кошелька «{}» не развёрнут исполнитель", w.name));
    };
    match w.executor_version.as_deref().and_then(code::Version::parse) {
        // Решение 395: v3 — надмножество v2 (executeNative тот же).
        Some(code::Version::V2 | code::Version::V3 | code::Version::V4) => {}
        Some(code::Version::V1) => {
            return Some(format!(
                "у кошелька «{}» исполнитель {x} версии v1 (executeNative нет) — переразверните исполнитель",
                w.name
            ));
        }
        None => {
            return Some(format!(
                "исполнитель {x} кошелька «{}» не сверен — нажмите «Проверить подключение» исполнителя",
                w.name
            ));
        }
    }
    let Ok(want) = ctx.addr("dex.native_wrapped") else {
        return Some(
            "не задана dex.native_wrapped — нативный BNB через исполнитель не идёт".into(),
        );
    };
    match w.executor_wrapped.as_deref().map(str::parse::<Address>) {
        Some(Ok(have)) if have == want => None,
        Some(Ok(have)) => Some(format!(
            "обёртка в исполнителе {x} кошелька «{}» — {have:#x}, а dex.native_wrapped — {want:#x}: нативный BNB через \
             этот исполнитель не идёт (переразверните исполнитель при верной dex.native_wrapped)",
            w.name
        )),
        _ => Some(format!(
            "обёртка исполнителя {x} кошелька «{}» не прочитана — нажмите «Проверить подключение» исполнителя",
            w.name
        )),
    }
}

/// Ревью t122: текст отказа «нативная монета через исполнитель» — один для сделки (trade.rs) и сухой сделки
/// (exec/dry_build.rs). Принимает: причину native_block. Возвращает: текст. Ошибок нет.
pub fn native_refusal(why: &str) -> String {
    format!("нативная монета через исполнитель не исполняется: {why}")
}

/// Недостающие разрешения ERC-20 исполнителю для вызова (wallet/approve_executor.rs). Решение 272: продажа нативного
/// BNB (value вызова > 0) платит BNB транзакции — разрешений нет. Принимает: вызов, кошелёк, исполнитель.
/// Возвращает: список разрешений. Ошибки: как approve_executor::missing.
pub async fn approvals(
    ctx: &Ctx,
    c: &call::ExecCall,
    wallet: Address,
    executor: Address,
) -> GatewayResult<Vec<super::super::wallet::approve::Approval>> {
    if !c.value().is_zero() {
        return Ok(Vec::new());
    }
    super::super::wallet::approve_executor::missing(ctx, c.token_in, wallet, executor, c.amount_in)
        .await
}

/// Пойдёт ли маршрут через исполнитель: графовый маршрут формы «бандл» (не один Universal Router, не прямой) при
/// заданном exec.executor_address, монета не нативная (решение 272: или нативная при исполнителе v2 — native_capable)
/// и каждый шаг — вид исполнителя (pools/exec_kind.rs::via_executor
/// и entry.rs::supported, та же
/// проверка, что исполнимость графа quote/graph_exec.rs); иначе — бандл, как раньше. Принимает: реестр пулов (ключ пула
/// менеджера — entry::fits, ревью 2 t181), маршрут, нативная ли монета. Возвращает: адрес исполнителя или None. Ошибки: как у address.
pub fn takes(
    ctx: &Ctx,
    reg: &Registry,
    route: &DexRoute,
    native: bool,
) -> GatewayResult<Option<Address>> {
    if (native && !native_capable(ctx))
        || route.mode != RouteMode::Graph
        || graph_route::graph_form(ctx, route) != GraphForm::Bundle
    {
        return Ok(None);
    }
    // Ревью 2 t181: та же проверка шага, что котировка (graph_executor.rs all_via — entry::fits): у пула менеджера
    // (V4 / Infinity) — ещё ключ записи реестра (нативная валюта, пара записи); у прочих видов fits = supported.
    let fits = |f: &families::Family, h: &crate::core::types::RouteHop| {
        let singleton = f.executor_kind.is_some_and(|k| k.singleton());
        if singleton {
            reg.entry(&h.pool).is_some_and(|e| entry::fits_ok(ctx, &e))
        } else {
            entry::supported(f, &h.pool).is_ok()
        }
    };
    let all = route.legs.iter().flat_map(|l| l.hops.iter()).all(|h| {
        families::classify(ctx, &h.family)
            .is_ok_and(|f| exec_kind::via_executor(ctx, f) && fits(f, h))
    });
    if !all {
        return Ok(None);
    }
    // Ревью t181 (Н3): то же правило выбора, что котировка (quote/graph_exec.rs bundle_first) — маршрут с шагом пула
    // менеджера, собираемый бандлом, идёт бандлом (как у кошелька v2).
    let fams: Vec<Vec<&'static families::Family>> = route
        .legs
        .iter()
        .map(|l| {
            l.hops
                .iter()
                .filter_map(|h| families::classify(ctx, &h.family).ok())
                .collect()
        })
        .collect();
    if super::super::quote::graph_exec::bundle_first(ctx, &fams, native) {
        return Ok(None);
    }
    address(ctx)
}
