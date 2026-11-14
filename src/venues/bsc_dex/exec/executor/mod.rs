//! Файл: venues/bsc_dex/exec/executor/mod.rs
//! Что делает: свой контракт-исполнитель ArbExecutor (решение 122, часть X; контракт — contracts/ArbExecutor.sol):
//! маршрут, которого не исполняет один Universal Router, — одной транзакцией через исполнитель, если задан
//! адрес исполнителя кошелька (решение 132; до него — exec.executor_address); не задан — как раньше (бандл, решение 84).
//! Здесь — адрес исполнителя кошелька и выбор пути.
//! Публичный интерфейс для новых семейств (часть F): kind.rs (Params, StepSpec, encode), call.rs (ExecCall, build).
//! Кто вызывает: exec/trade.rs (выбор пути), exec/dry_build.rs (сухая сделка), quote/approve_gas.rs (разрешение),
//! venue.rs (развёртывание).
//! От чего зависит: exec/executor/*, quote/graph_route.rs (graph_form), ctx.rs.
//! Настройки: нет (адрес исполнителя — у кошелька, экран «Маппинг кошельков»).

pub mod abi;
pub mod call;
pub mod code;
pub mod deploy;
pub mod dry;
pub mod entry;
pub mod fact;
pub mod kind;
pub mod onchain;
pub mod remeasure;
pub mod sim;
pub mod trade;

use alloy_primitives::Address;

use crate::core::types::{DexRoute, RouteMode};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::pools::{exec_kind, families};
use super::super::quote::graph_route::{self, GraphForm};

/// Адрес развёрнутого исполнителя кошелька окружения (решение 132: у каждого кошелька свой — ctx_wallet.rs).
/// Принимает: окружение. Возвращает: None — не развёрнут (маршруты через разные роутеры идут бандлом, как раньше).
/// Ошибки: Internal — адрес записан не адресом.
pub fn address(ctx: &Ctx) -> GatewayResult<Option<Address>> {
    ctx.executor()
}

/// Решение 272: идёт ли нативный BNB монетой сделки через исполнитель кошелька окружения — у кошелька задан
/// исполнитель и его сверенная версия (onchain.rs → запись кошелька) — v2 (executeNative). Не сверена или v1 — нет
/// (поведение до решения 272). Ошибок нет.
pub fn native_capable(ctx: &Ctx) -> bool {
    ctx.wallet().is_ok_and(|w| {
        w.executor.is_some()
            && w.executor_version.as_deref().and_then(code::Version::parse) == Some(code::Version::V2)
    })
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
    super::super::wallet::approve_executor::missing(ctx, c.token_in, wallet, executor, c.amount_in).await
}

/// Пойдёт ли маршрут через исполнитель: графовый маршрут формы «бандл» (не один Universal Router, не прямой) при
/// заданном exec.executor_address, монета не нативная (решение 272: или нативная при исполнителе v2 — native_capable)
/// и каждый шаг — вид исполнителя (pools/exec_kind.rs::via_executor
/// и entry.rs::supported, та же
/// проверка, что исполнимость графа quote/graph_exec.rs); иначе — бандл, как раньше. Принимает: маршрут, нативная ли
/// монета. Возвращает: адрес исполнителя или None. Ошибки: как у address.
pub fn takes(ctx: &Ctx, route: &DexRoute, native: bool) -> GatewayResult<Option<Address>> {
    if (native && !native_capable(ctx))
        || route.mode != RouteMode::Graph
        || graph_route::graph_form(ctx, route) != GraphForm::Bundle
    {
        return Ok(None);
    }
    let all = route.legs.iter().flat_map(|l| l.hops.iter()).all(|h| {
        families::classify(ctx, &h.family)
            .is_ok_and(|f| exec_kind::via_executor(ctx, f) && entry::supported(f, &h.pool).is_ok())
    });
    if !all {
        return Ok(None);
    }
    address(ctx)
}
