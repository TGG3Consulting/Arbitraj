//! Файл: venues/bsc_dex/exec/cycle_gate.rs
//! Что делает: гейт и выбор формы исполнения цикла DEX ↔ DEX (решения 86, 124).
//! - Гейт dex_dex.execute (решение 86): не задано / нет — цикл только сигнал, сделки нет («всё под контролем человека»).
//! - Форма (решение 124): (b) свой исполнитель ArbExecutor одной транзакцией — если задан exec.executor_address и КАЖДЫЙ
//!   шаг цикла — вид шага исполнителя (pools/exec_kind.rs::via_executor, exec/executor/entry.rs::supported; та же
//!   проверка, что у маршрута ноги — exec/executor/mod.rs::takes) — предпочтительно: атомарна сама, любые семейства,
//!   без пыли; (a) бандл свопов cycle_bundle (решения 84, 86) — если все шаги идут через Universal Router (tradable, не
//!   форк, задан dex.universal_router) и ВСЕ включённые каналы отправки бандловые (решение 121 B3,
//!   send::planned_bundle_ok); иначе — NoRoute с причинами обеих форм (цикл остаётся сигналом).
//!
//! Кто вызывает: exec/{cycle_trade, cycle_dry, cycle_bundle}.rs, quote/cycle_cost.rs.
//! От чего зависит: exec/executor/{mod, entry}.rs, pools/{exec_kind, families}.rs, quote/route_calc.rs (Leg), send/mod.rs.
//! Настройки: dex_dex.execute, exec.executor_address [bsc_dex]; dex.universal_router [DEX]; channel.* (через send).

use alloy_primitives::Address;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::exec_kind;
use super::super::quote::route_calc::Leg;
use super::super::send;
use super::executor::{self, entry};

/// Форма исполнения цикла.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// Одна транзакция через исполнитель ArbExecutor по этому адресу.
    Executor(Address),
    /// Атомарный бандл свопов через Universal Router (решение 84).
    Bundle,
}

impl Form {
    /// Подпись формы для журнала, карточки и сухой сделки. Ошибок нет.
    pub fn text(&self) -> String {
        match self {
            Form::Executor(x) => format!("цикл: исполнитель {x:#x}"),
            Form::Bundle => "цикл: бандл свопов".into(),
        }
    }
}

/// Включено ли АВТО-ИСПОЛНЕНИЕ циклов DEX ↔ DEX (решение 86). Не задано — выключено: цикл только сигнал. Ошибок нет.
pub fn execute_enabled(ctx: &Ctx) -> bool {
    ctx.flag("dex_dex.execute").unwrap_or(false)
}

/// Гейт исполнения цикла: выключено (по умолчанию) — сделки нет (сигнал). Ошибки: NoRoute — гейт закрыт.
pub fn require_execute(ctx: &Ctx) -> GatewayResult<()> {
    if execute_enabled(ctx) {
        Ok(())
    } else {
        Err(ctx.err(
            ErrorCode::NoRoute,
            "авто-исполнение циклов DEX ↔ DEX выключено (dex_dex.execute) — цикл только сигнал",
        ))
    }
}

/// Хоп цикла исполним бандлом v1 (через Universal Router). Ошибки: NoRoute — не-tradable семейство или форк (classic
/// V2 Router — следующая фаза); NotConfigured — у DEX не задан dex.universal_router.
pub fn guard_ur(ctx: &Ctx, leg: &Leg) -> GatewayResult<()> {
    let f = leg.entry.family;
    if !f.tradable {
        let why = format!(
            "цикл проходит через {} без своего пути исполнения — это сигнал, бандлом не исполняется",
            f.id
        );
        return Err(ctx.err(ErrorCode::NoRoute, why));
    }
    if f.fork {
        let why = format!(
            "хоп цикла через форк {} (свой classic V2 Router) в бандле цикла v1 не собирается — следующая фаза",
            f.id
        );
        return Err(ctx.err(ErrorCode::NoRoute, why));
    }
    ctx.dex_addr("dex.universal_router", f.id)?;
    Ok(())
}

/// Выбор формы: исполнитель, иначе бандл, иначе отказ с причинами. Принимает: ноги цикла. Ошибки: NoRoute (обе формы
/// не годятся — причины обеих), Internal (адрес исполнителя кошелька не читается).
pub fn form(ctx: &Ctx, legs: &[Leg]) -> GatewayResult<Form> {
    let exec_why = match executor::address(ctx)? {
        None => format!(
            "у кошелька «{}» не развёрнут исполнитель ArbExecutor (экран «Маппинг кошельков»)",
            ctx.wallet_name()
        ),
        Some(x) => match legs.iter().find_map(|l| step_why(ctx, l)) {
            None => return Ok(Form::Executor(x)),
            Some(w) => w,
        },
    };
    let bundle_why = match legs.iter().find_map(|l| guard_ur(ctx, l).err()) {
        Some(e) => e.message,
        None => match send::planned_bundle_ok(ctx) {
            Ok(()) => return Ok(Form::Bundle),
            Err(w) => w,
        },
    };
    let why = format!("цикл не исполним: исполнитель — {exec_why}; бандл — {bundle_why}");
    Err(ctx.err(ErrorCode::NoRoute, why))
}

/// Почему шаг не идёт исполнителем (None — идёт): у вида пула нет шага исполнителя или пул внутри менеджера.
fn step_why(ctx: &Ctx, l: &Leg) -> Option<String> {
    let f = l.entry.family;
    if !exec_kind::via_executor(ctx, f) {
        return Some(format!("{}: у вида пула нет шага исполнителя", f.id));
    }
    entry::supported(f, &l.entry.pool.reference).err()
}
