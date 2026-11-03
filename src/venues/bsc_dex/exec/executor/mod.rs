//! Файл: venues/bsc_dex/exec/executor/mod.rs
//! Что делает: свой контракт-исполнитель ArbExecutor (решение 122, часть X; контракт — contracts/ArbExecutor.sol):
//! маршрут, которого не исполняет один Universal Router, — одной транзакцией через исполнитель, если задан
//! exec.executor_address; не задан — как раньше (бандл, решение 84). Здесь — адрес из настроек и выбор пути.
//! Публичный интерфейс для новых семейств (часть F): kind.rs (Params, StepSpec, encode), call.rs (ExecCall, build).
//! Кто вызывает: exec/trade.rs (выбор пути), exec/dry_build.rs (сухая сделка), quote/approve_gas.rs (разрешение),
//! venue.rs (развёртывание).
//! От чего зависит: exec/executor/*, quote/graph_route.rs (graph_form), ctx.rs.
//! Настройки: exec.executor_address [bsc_dex] (необязательная).

pub mod abi;
pub mod call;
pub mod code;
pub mod deploy;
pub mod dry;
pub mod entry;
pub mod fact;
pub mod kind;
pub mod sim;
pub mod trade;

use alloy_primitives::Address;

use crate::core::types::{DexRoute, RouteMode};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::quote::graph_route::{self, GraphForm};

/// Адрес развёрнутого исполнителя (exec.executor_address). Принимает: окружение. Возвращает: None — не задан (маршруты
/// через разные роутеры идут бандлом, как раньше). Ошибки: Internal — значение не читается как адрес.
pub fn address(ctx: &Ctx) -> GatewayResult<Option<Address>> {
    match ctx.addr("exec.executor_address") {
        Ok(a) => Ok(Some(a)),
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(None),
        Err(e) => Err(e),
    }
}

/// Пойдёт ли маршрут через исполнитель: графовый маршрут формы «бандл» (не один Universal Router, не прямой) при
/// заданном exec.executor_address. Возвращает: адрес исполнителя или None. Ошибки: как у address.
pub fn takes(ctx: &Ctx, route: &DexRoute) -> GatewayResult<Option<Address>> {
    if route.mode != RouteMode::Graph || graph_route::graph_form(ctx, route) != GraphForm::Bundle {
        return Ok(None);
    }
    address(ctx)
}
