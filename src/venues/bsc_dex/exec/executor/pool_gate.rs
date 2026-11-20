//! Файл: venues/bsc_dex/exec/executor/pool_gate.rs
//! Что делает: решение 401 (t186) — одно правило исполнимости шага пула менеджера (Uniswap V4, Infinity CL / Bin /
//! Stable) исполнителем v3 для котировки и сделки (entry.rs fits и singleton → их зовут quote/graph_executor.rs all_via,
//! quote/graph_fit.rs, exec/executor/mod.rs takes, exec/cycle_gate.rs, check/probe_exec.rs, call::build). Дополняет
//! приёмку класса по записи симуляции (решение 400, live_proof.rs) фактом пробы ЭТОГО пула через исполнитель кошелька
//! окружения (pool_pass.rs):
//! - факт «прошёл обе стороны» — шаг исполним, даже если класс записью не принят (крючок, Infinity Stable, Bin);
//! - факт «отвергнут» (проба цикла крючков или откат пробы сделки) — шаг не исполним, даже если класс принят; маршрут
//!   идёт прежним путём (бандл / Universal Router — правило bundle_first), иначе сигнал с этой причиной;
//! - фактов нет или «не проверено» — как решение 400: только принятый класс.
//!
//! Кошелёк — окружения (ctx_wallet.rs: счёт сделки или строка «Пробы»); у кошелька без исполнителя фактов нет.
//! Ревью t186 (Н1): решение (decide, admitted) — без замков общих структур и без строк: адрес исполнителя — снимок
//! кошельков (Ctx::exec_info), вид итога пула — снимок pool_pass::kind, класс — live_proof::accepted (OnceLock). Текст
//! причины строит только admits (лениво, при отказе: журнал, сигнал, сделка).
//! Кто вызывает: exec/executor/{entry, call}.rs.
//! От чего зависит: exec/executor/{live_proof, pool_pass}.rs, pools/{registry, families, exec_kind}.rs, ctx.rs,
//! ctx_wallet.rs (exec_info).
//! Настройки: нет.

use alloy_primitives::Address;

use super::super::super::ctx::Ctx;
use super::super::super::pools::exec_kind::ExecKind;
use super::super::super::pools::families::Proto;
use super::super::super::pools::registry::Entry;
use super::pool_pass::{self, Gated, Kind};

/// Чем шаг допущен.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    /// Не шаг менеджера или класс принят записью (решение 400), фактов против пула нет.
    Record,
    /// Класс записью не принят — пул прошёл пробу через исполнитель кошелька (решение 401).
    Pass,
}

/// Почему не допущен (без текста — текст строит admits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refuse {
    /// Класс записью не принят, пул пробу не прошёл (не было или «не проверено»).
    Class,
    /// Пул отвергнут пробой или откатом пробы сделки.
    Rejected,
}

/// Горячий путь (ревью t186, Н1): решение без замков общих структур и без строк. Принимает: окружение, пул, вид шага,
/// крючок ключа. Возвращает: чем допущен или почему нет. Ошибок иных нет.
pub fn decide(ctx: &Ctx, e: &Entry, k: ExecKind, hooks: Address) -> Result<Admit, Refuse> {
    let stable = e.family.proto == Proto::InfinityStable;
    let record = super::live_proof::accepted(k, stable, hooks);
    if super::live_proof::number(k).is_none() {
        return Ok(Admit::Record);
    }
    let by_record = if record {
        Ok(Admit::Record)
    } else {
        Err(Refuse::Class)
    };
    let Some((x, _)) = ctx.exec_info() else {
        return by_record;
    };
    match pool_pass::kind(ctx.place.id, x, &e.pool.reference) {
        Some(Kind::Passed) if record => Ok(Admit::Record),
        Some(Kind::Passed) => Ok(Admit::Pass),
        Some(Kind::Rejected) => Err(Refuse::Rejected),
        Some(Kind::Unchecked) | None => by_record,
    }
}

/// Горячий путь: допущен ли шаг (decide без причины). Ошибок нет.
pub fn admitted(ctx: &Ctx, e: &Entry, k: ExecKind, hooks: Address) -> bool {
    decide(ctx, e, k, hooks).is_ok()
}

/// Исполним ли шаг пула исполнителем кошелька окружения — с причиной отказа (строится только при отказе). Принимает:
/// окружение, пул, вид шага, крючок ключа. Возвращает: чем допущен или причину. Ошибок иных нет.
pub fn admits(ctx: &Ctx, e: &Entry, k: ExecKind, hooks: Address) -> Result<Admit, String> {
    decide(ctx, e, k, hooks).map_err(|r| refusal(ctx, e, k, hooks, r))
}

/// Текст отказа (не горячий путь: полный факт и имя кошелька). Ошибок нет.
fn refusal(ctx: &Ctx, e: &Entry, k: ExecKind, hooks: Address, r: Refuse) -> String {
    let fact = ctx
        .exec_info()
        .and_then(|(x, _)| pool_pass::get(ctx.place.id, x, &e.pool.reference));
    let line = pool_pass::line(&ctx.wallet_name(), fact.as_ref());
    match r {
        Refuse::Rejected => line,
        Refuse::Class => {
            let stable = e.family.proto == Proto::InfinityStable;
            let class = super::live_proof::admits(k, stable, hooks)
                .err()
                .unwrap_or_default();
            format!("{class}; этот пул: {line}")
        }
    }
}

/// Пул шага менеджера для вызова исполнителя (pool_pass::on_sim): ссылка и крючок ключа. None — не шаг менеджера или
/// нет ключа. Ошибок нет.
pub fn gated(e: &Entry) -> Option<Gated> {
    e.family.executor_kind.filter(|k| k.singleton())?;
    let hooks = e.key.as_ref()?.hooks();
    Some(Gated {
        pool: e.pool.reference.clone(),
        hooks,
    })
}
