//! Файл: core/calc_detail.rs
//! Что делает: решение 265 — части замера этапов, добавленные «Временем по шагам» (делистинг, закрытые входы, свежесть
//! налогов, исполнимый объём у исполнителя, границы / точки / уточнение / расчёт объёма подбора). Пишутся только в
//! подробный замер (calc_timing.rs Timing::detailed — пересчёт сделки при log.step_timing = да); в замере расчёта
//! кандидата (решение 129, «Кандидаты») и без замера работа идёт как есть — отчёт «Кандидатов» как до решения 265.
//! Кто вызывает: core/profit.rs, core/size_search.rs, core/size_memo.rs, core/costs.rs (detail — имя части доли глубины).
//! От чего зависит: core/calc_timing.rs.
//! Настройки: нет (включатель log.step_timing читает executor.rs, step_timing.rs).

use std::future::Future;

pub use super::calc_timing::detail;
use super::calc_timing::{stage, stage_sync};

/// Решение 265: часть только подробного замера — в обычном (расчёт кандидата, решение 129) и без замера работа идёт
/// как есть, «Кандидаты» не меняются. Принимает, возвращает: как stage.
pub async fn stage_detail<F: Future>(name: impl FnOnce() -> String, work: F) -> F::Output {
    if detail() {
        stage(name, work).await
    } else {
        work.await
    }
}

/// То же для синхронного шага. Принимает, возвращает: как stage_sync.
pub fn stage_sync_detail<T>(name: impl FnOnce() -> String, work: impl FnOnce() -> T) -> T {
    if detail() {
        stage_sync(name, work)
    } else {
        work()
    }
}
