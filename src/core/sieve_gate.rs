//! Файл: core/sieve_gate.rs
//! Что делает: решения 372, 374, 377 — отсев по границе в начале расчёта кандидата (candidates.rs calc_one), одним
//! вызовом:
//! - sieve.rs check (только память) и счётчик итога (sieve_log.rs count);
//! - не отсечён (граница не исключает сделку или метод не действует) — полный расчёт, ему отдаётся уже построенный
//!   контекст направления (ревью t162, Н7: DirCtx с замером σ не строится второй раз), отметка частоты снимается;
//! - отсечён, а интервал показа по направлению не прошёл (sieve_rate.rs, sieve.row_interval_ms; ревью t162, Н4) — только
//!   счётчик, расчёта нет;
//! - отсечён и показ — доля sieve.audit_share идёт в контрольный ОБЫЧНЫЙ полный расчёт (sieve_audit.rs; ревью t162, Н3),
//!   остальные — строка, эпизод и журнал отсева (sieve_log.rs cut); в данные отсева кладутся то, что у отсечённого
//!   есть без полного расчёта (ревью t162, Н11): сравнение σ (DirCtx.sigma_check, решение 358 — σ-часть, без прибыли
//!   по σ_T: её даёт только полный расчёт) и signal_note (cand_signal_note.rs, решение 359).
//!
//! Сухой режим без бюджета — отсева нет (причину даст decide, как было).
//! Кто вызывает: core/candidates.rs (calc_one).
//! От чего зависит: sieve.rs, sieve_log.rs, sieve_rate.rs, sieve_audit.rs, cand_signal_note.rs, dry.rs, dirctx.rs.
//! Настройки: sieve.row_interval_ms (sieve_rate.rs), sieve.audit_share (sieve_audit.rs); через sieve.rs — пороги и прочее.

use crate::core::types::Direction;

use super::dirctx::DirCtx;
use super::dry::Mode;
use super::engine::Core;
use super::sieve::{Checked, Sieve};

/// Что дальше с кандидатом.
pub enum Gate {
    /// Отсечён (показан или в пределе частоты) — расчёта нет.
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
        super::sieve_rate::clear(d);
        return Gate::Calc { cx, audit: false };
    };
    if !super::sieve_rate::due(core, d) {
        return Gate::Done;
    }
    if super::sieve_audit::pick(core) {
        super::sieve_rate::clear(d);
        super::sieve_log::bump("audit");
        return Gate::Calc { cx, audit: true };
    }
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
    Box::pin(super::sieve_log::cut(core, d, reason, data)).await;
    Gate::Done
}
