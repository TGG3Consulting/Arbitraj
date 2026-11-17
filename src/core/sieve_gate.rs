//! Файл: core/sieve_gate.rs
//! Что делает: решения 372, 374, 377 — отсев по границе в начале расчёта кандидата (candidates.rs calc_one), одним
//! вызовом:
//! - sieve.rs check (только память) и счётчик итога (sieve_log.rs count);
//! - не отсечён (граница не исключает сделку или метод не действует) — полный расчёт, ему отдаётся уже построенный
//!   контекст направления (ревью t162, Н7: DirCtx с замером σ не строится второй раз);
//! - отсечён — доля sieve.audit_share ВСЕХ отсечённых идёт в контрольный ОБЫЧНЫЙ полный расчёт (sieve_audit.rs; ревью
//!   t162, Н3; решение 378: выбор не зависит от показа);
//! - отсечён, не контроль — журнал отсева, а строка и эпизод — только при записи журнала (sieve_log.rs cut, решение 386:
//!   смена вида причины или обновление строки повторов; без таймера — sieve.row_interval_ms удалена); в данные
//!   показываемого отсева (sieve_log.rs due — только тогда, ревью t171 Н9) кладутся то, что у отсечённого есть без
//!   полного расчёта (ревью t162, Н11): сравнение σ (DirCtx.sigma_check, решение 358 — σ-часть, без прибыли по σ_T: её
//!   даёт только полный расчёт) и signal_note (cand_signal_note.rs, решение 359). В повторе, который строка журнала
//!   получает отложенно (без показа), этих двух полей нет — только данные границы.
//!
//! Сухой режим без бюджета — отсева нет (причину даст decide, как было).
//! Кто вызывает: core/candidates.rs (calc_one).
//! От чего зависит: sieve.rs, sieve_log.rs, sieve_audit.rs, cand_signal_note.rs, dry.rs, dirctx.rs.
//! Настройки: sieve.audit_share (sieve_audit.rs); через sieve_log.rs — journal.repeat_flush_ms; через sieve.rs — пороги.

use crate::core::types::Direction;

use super::dirctx::DirCtx;
use super::dry::Mode;
use super::engine::Core;
use super::sieve::{Checked, Sieve};

/// Что дальше с кандидатом.
pub enum Gate {
    /// Отсечён — расчёта нет.
    Done,
    /// Полный расчёт: готовый контекст направления (None — строится расчётом) и признак контрольного расчёта.
    Calc {
        cx: Option<Box<DirCtx>>,
        audit: bool,
    },
}

/// Что делает: отсев кандидата и всё, что с ним связано (см. шапку). Принимает: ядро, направление, режим расчёта.
/// Возвращает: Gate. Ошибок нет.
pub async fn run(core: &Core, d: &Direction, mode: &Mode) -> Gate {
    if matches!(mode, Mode::Dry(Err(_))) {
        return Gate::Calc {
            cx: None,
            audit: false,
        };
    }
    let Checked { sieve, cx } = Box::pin(super::sieve::check(core, d)).await;
    super::sieve_log::count(&sieve);
    let Sieve::Cut { reason, mut data } = sieve else {
        return Gate::Calc { cx, audit: false };
    };
    // Решение 378 (Тигран 17.11): контроль выбирается из ВСЕХ отсечённых, от показа не зависит.
    if super::sieve_audit::pick(core) {
        super::sieve_log::bump("audit");
        return Gate::Calc { cx, audit: true };
    }
    // Решение 386 (ревью t171, Н9): данные показа — только если отсев сейчас покажется (запись журнала пишется).
    let shown = super::sieve_log::due(core, d, &reason);
    if shown {
        if let Some(sc) = cx.as_ref().and_then(|c| c.sigma_check.as_ref())
            && let Ok(v) = serde_json::to_value(sc)
        {
            data["sigma_check"] = v;
        }
        let st = super::cand_signal_note::start(core, d);
        if let Some(note) =
            super::cand_signal_note::data(None, &st).and_then(|v| v.get("signal_note").cloned())
        {
            data["signal_note"] = note;
        }
    }
    Box::pin(super::sieve_log::cut(core, d, reason, data, shown)).await;
    Gate::Done
}
