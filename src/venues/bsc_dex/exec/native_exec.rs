//! Файл: venues/bsc_dex/exec/native_exec.rs
//! Что делает: решение 272 — нативный BNB монетой сделки через контракт-исполнитель v2 (executeNative,
//! contracts/ArbExecutor.sol). Правило одно: пул на стороне BNB исполним шагом исполнителя (pools/exec_kind.rs::via_executor)
//! И исполнитель кошелька окружения сверен как v2 (exec/executor/mod.rs::native_capable). Тогда BNB идёт через пулы
//! форков V2 / V3, Algebra, Solidly, DODO, StableSwap, Maverick, Slipstream — всех видов с шагом исполнителя. Исполнитель
//! v1 или не сверен — false: поведение до решения 272 без изменений (exec/native.rs::pool_ok, trade_path — те же отказы).
//! Кто вызывает: quote/graph_route.rs (фильтр путей BNB), quote/graph_exec.rs (исполнимость), check/native_probe.rs
//! (проба газа обёртки через исполнитель — check/native_probe_exec.rs).
//! От чего зависит: exec/native.rs (pool_ok), exec/executor/mod.rs (native_capable), pools/{exec_kind, families}.rs.
//! Настройки: нет (адрес и версия исполнителя — у кошелька, экран «Маппинг кошельков»).

use crate::core::types::PoolKind;

use super::super::ctx::Ctx;
use super::super::pools::exec_kind;
use super::super::pools::families::Family;
use super::{executor, native};

/// Идёт ли BNB через пул этого вида шагом исполнителя v2 (пул на стороне BNB — касается обёртки). Решение 395: шаг пула
/// внутри менеджера (UNI_V4 / INF_*, ExecKind::singleton) на стороне BNB не берётся — как до решения 395 (газ обёртки
/// через него не мерится); хоп менеджера, не касающийся BNB, этим правилом не закрыт (ревью 2 t181).
/// Принимает: окружение, вид пула. Ошибок нет.
pub fn exec_ok(ctx: &Ctx, f: &Family) -> bool {
    exec_kind::via_executor(ctx, f)
        && !f.executor_kind.is_some_and(|k| k.singleton())
        && executor::native_capable(ctx)
}

/// Годится ли пул на стороне BNB: V2/V3 со своим путём (как до решения 272, native::pool_ok) или шаг исполнителя v2.
/// Принимает: окружение, вид ядра пула, вид пула (семейство). Ошибок нет.
pub fn edge_ok(ctx: &Ctx, kind: PoolKind, f: &Family) -> bool {
    native::pool_ok(kind) || exec_ok(ctx, f)
}
