//! Файл: core/auto_pause.rs
//! Что делает: решение 225 (Тигран, 08.11.2026) — «пауза-плей автоматически — только когда на паузу поставил сам софт;
//! паузу, поставленную вручную, снимает только человек». Здесь — кто поставил паузу и почему (чистая логика над
//! Risk, без ядра):
//! - пауза площадки (Risk.venue_paused) от софта — отметка Risk.venue_auto с видом: Errors (risk.max_errors_in_row,
//!   risk::on_error), Rule (правило шлюза «стоп площадка»), Outcome («исход неизвестен» — отметка «только человек»:
//!   никакая автоматика такую площадку не снимает, решение 64; сам бот исход не узнаёт — см. DECISIONS 225);
//! - «Стоп» системы от софта — отметка Risk.mode_auto с видом: DailyTotal (общий лимит дня, daily_limit.rs) или Rule
//!   (правило шлюза «стоп всё»; площадка, чья ошибка вызвала стоп) и режимом до остановки.
//!   Человек сменил режим (risk::set_mode) или снял паузу площадки (risk::resume_venue) — отметка снимается, автоматика
//!   такую паузу больше не трогает. Пауза по убытку дня площадки — своя отметка Risk.loss_paused (daily_venue_limit.rs),
//!   снимается с новым днём (day_roll.rs).
//! - отказ вывода / неверный адрес / вывод заблокирован (WithdrawRejected, InvalidAddress, WithdrawLocked) — паузу НЕ
//!   ставят вовсе (вывод не повторяем, продаём на месте — решение 220б): не считаются ошибкой подряд (risk_count.rs),
//!   правило шлюза «стоп площадка» / «стоп всё» для них — предупреждение без паузы (rules_stop.rs).
//!   Снятие по проверке подключения — auto_resume.rs, с новым днём — day_roll.rs.
//!
//! Кто вызывает: risk.rs (on_error, resume_venue, set_mode), rules_stop.rs, rules_apply.rs, daily_limit.rs,
//! auto_resume.rs, day_roll.rs, risk_count.rs (no_pause), тесты.
//! От чего зависит: risk.rs (Risk, Mode), gateway/errors.rs.
//! Настройки: нет (интервалы — auto_resume.rs).

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::core::types::VenueId;
use crate::gateway::errors::ErrorCode;

use super::risk::{Mode, Risk};

/// Вид софтовой паузы площадки. Порядок — сила: более сильная отметка заменяет слабую, обратно — нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Kind {
    /// N ошибок подряд (risk.max_errors_in_row): снятие — после risk.errors_resume_after_s проверкой подключения.
    Errors,
    /// Правило шлюза «стоп площадка»: снятие — проверкой подключения.
    Rule,
    /// «Исход неизвестен»: только человек.
    Outcome,
}

/// Отметка софтовой паузы площадки.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VenueMark {
    pub kind: Kind,
    /// Когда софт поставил паузу (первая постановка; повторные ошибки время не сдвигают).
    pub since: DateTime<Utc>,
}

/// Кто остановил систему.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum ModeKind {
    /// Общий лимит убытка дня (risk.max_daily_loss_total_usd): снятие — с новым днём (UTC).
    DailyTotal,
    /// Правило шлюза «стоп всё» из-за ошибки площадки `venue`: снятие — проверкой подключения этой площадки.
    Rule { venue: VenueId },
}

/// Отметка «Стоп» системы от софта.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModeMark {
    pub kind: ModeKind,
    /// Режим до остановки — его и вернёт автоснятие.
    pub prev: Mode,
    /// Причина режима до остановки (у «Пауза» человека — его причина).
    #[serde(skip)]
    pub prev_reason: Option<String>,
    pub since: DateTime<Utc>,
}

/// Коды, по которым пауза не ставится вовсе (решение 225; вывод не повторяем, продаём на месте — решение 220б).
/// Принимает: код ошибки. Ошибок нет.
pub fn no_pause(code: ErrorCode) -> bool {
    // Решение 230: тот же перечень — у формы и API правил (gateway/rules.rs withdraw_refusal).
    crate::gateway::rules::withdraw_refusal(code)
}

/// Ставит паузу площадки от софта: причина и отметка вида. Правила отметки: площадки не было на паузе — отметка вида
/// `kind`; уже была софтовая отметка — остаётся более сильная (время первой постановки сохраняется); пауза стояла без
/// отметки (убыток дня, сверка, человек) — отметки нет, кроме Outcome: «исход неизвестен» ставит отметку «только
/// человек» поверх любой паузы. При отметке Outcome причину другая пауза не перезаписывает. Решение 221: пауза сверки
/// (Risk.reconcile_venues) — тоже софтовая: её отметка снимается (risk::forget_reconcile — сверка эту паузу больше не
/// снимает), а площадка считается «не на паузе» — получает отметку вида `kind` (снимет auto_resume.rs или человек).
/// Принимает: состояние риска (под блокировкой), площадку, причину, вид, время. Возвращает: true — пауза новая или
/// отметка стала сильнее (для уведомления). Ошибок нет.
pub fn pause_venue(r: &mut Risk, v: &VenueId, why: String, kind: Kind, now: DateTime<Utc>) -> bool {
    let reconcile = super::risk::forget_reconcile(r, v);
    let was = r.venue_paused.contains_key(v) && !reconcile;
    let prev = r.venue_auto.get(v).cloned();
    let mark = match (&prev, kind) {
        (_, Kind::Outcome) => Some(VenueMark {
            kind,
            since: prev
                .as_ref()
                .filter(|m| m.kind == Kind::Outcome)
                .map_or(now, |m| m.since),
        }),
        (Some(m), _) => Some(VenueMark {
            kind: m.kind.max(kind),
            since: m.since,
        }),
        (None, _) if !was => Some(VenueMark { kind, since: now }),
        (None, _) => None,
    };
    if prev.as_ref().is_none_or(|m| m.kind != Kind::Outcome) || kind == Kind::Outcome {
        r.venue_paused.insert(v.clone(), why);
    }
    let changed = !was || prev.as_ref().map(|m| m.kind) != mark.as_ref().map(|m| m.kind);
    match mark {
        Some(m) => r.venue_auto.insert(v.clone(), m),
        None => r.venue_auto.remove(v),
    };
    changed
}

/// Пауза площадки снимается только человеком («исход неизвестен»). Ошибок нет.
pub fn human_only(r: &Risk, v: &VenueId) -> bool {
    r.venue_auto.get(v).is_some_and(|m| m.kind == Kind::Outcome)
}

/// Пора ли проверять подключение для автоснятия. Errors — после ожидания `wait_errors_s` с постановки (не задано —
/// никогда, снимает человек); Rule — на каждом круге; Outcome — никогда.
/// Принимает: отметку, сейчас, ожидание для Errors (секунды). Ошибок нет.
pub fn due(m: &VenueMark, now: DateTime<Utc>, wait_errors_s: Option<u64>) -> bool {
    match m.kind {
        Kind::Errors => wait_errors_s
            .and_then(|w| i64::try_from(w).ok())
            .is_some_and(|w| (now - m.since).num_seconds() >= w),
        Kind::Rule => true,
        Kind::Outcome => false,
    }
}

/// Снимает софтовую паузу площадки, если отметка та же, что была при проверке (не сменилась и не снята человеком):
/// пауза, счётчик ошибок подряд, отметка. Принимает: состояние риска, площадку, отметку на момент проверки.
/// Возвращает: true — снята. Ошибок нет.
pub fn lift_venue(r: &mut Risk, v: &VenueId, seen: &VenueMark) -> bool {
    let same = r.venue_auto.get(v) == Some(seen) && seen.kind != Kind::Outcome;
    if !same || !r.venue_paused.contains_key(v) {
        return false;
    }
    r.venue_paused.remove(v);
    r.errors_in_row.remove(v);
    r.venue_auto.remove(v);
    true
}

/// Запоминает «Стоп» системы от софта (вызывать сразу после смены режима на Stopped). Режим до остановки уже «Стоп» —
/// отметки нет (возвращать нечего). Принимает: состояние риска, вид, режим и причину до остановки, время. Ошибок нет.
pub fn mark_mode(
    r: &mut Risk,
    kind: ModeKind,
    prev: Mode,
    prev_reason: Option<String>,
    now: DateTime<Utc>,
) {
    r.mode_auto = (prev != Mode::Stopped).then_some(ModeMark {
        kind,
        prev,
        prev_reason,
        since: now,
    });
}

/// Возвращает режим до софтового «Стоп», если отметка та же и система всё ещё в «Стоп». Причина: «В работе» — `why`;
/// «Пауза» — её прежняя причина. Принимает: состояние риска, отметку на момент проверки, текст. Возвращает: режим,
/// который вернулся; None — отметка сменилась или режим меняли. Ошибок нет.
pub fn restore_mode(r: &mut Risk, seen: &ModeMark, why: String) -> Option<Mode> {
    if r.mode_auto.as_ref() != Some(seen) || r.mode != Mode::Stopped {
        return None;
    }
    let m = r.mode_auto.take()?;
    r.mode = m.prev;
    r.reason = if m.prev == Mode::Running {
        Some(why)
    } else {
        m.prev_reason
    };
    r.daily_stop = false;
    Some(m.prev)
}
