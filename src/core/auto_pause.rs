//! Файл: core/auto_pause.rs
//! Что делает: решение 225 (Тигран, 08.11.2026) — «пауза-плей автоматически — только когда на паузу поставил сам софт;
//! паузу, поставленную вручную, снимает только человек». Здесь — кто поставил паузу и почему (чистая логика над
//! Risk, без ядра):
//! - пауза площадки (Risk.venue_paused) от софта — отметка Risk.venue_auto с видом: Errors (risk.max_errors_in_row,
//!   risk::on_error), Rule (правило шлюза «стоп площадка»), Outcome («исход неизвестен»: проверка подключения и сверка
//!   её не снимают, решение 64; решение 231 — снимает только выяснение исхода ботом, когда выяснены все такие операции
//!   площадки (outcome_open.rs, outcome_resolve.rs), иначе человек);
//! - «Стоп» системы от софта — отметка Risk.mode_auto с видом: DailyTotal (общий лимит дня, daily_limit.rs) или Rule
//!   (правило шлюза «стоп всё»; площадка, чья ошибка вызвала стоп) и режимом до остановки.
//!   Человек сменил режим (risk::set_mode) или снял паузу площадки (risk::resume_venue) — отметка снимается, автоматика
//!   такую паузу больше не трогает. Пауза по убытку дня — не пауза площадки, а пауза пары «площадка × тип направления»
//!   (Risk.pair_paused, решение 258, daily_venue_limit.rs), снимается с новым днём (day_roll.rs).
//! - отказ вывода / неверный адрес / вывод заблокирован / проблема аккаунта (WithdrawRejected, InvalidAddress,
//!   WithdrawLocked, AccountRestricted — решение 253, дополнение t82) — паузу НЕ
//!   ставят вовсе (вывод не повторяем, продаём на месте — решение 220б): не считаются ошибкой подряд (risk_count.rs),
//!   правило шлюза «стоп площадка» / «стоп всё» для них — предупреждение без паузы (rules_stop.rs).
//!   Снятие по проверке подключения — auto_resume.rs, с новым днём — day_roll.rs.
//! - Решение 253, дополнение t85 (Тигран 11.11): исчерпан лимит вывода аккаунта (WithdrawLimit) — пауза площадки с
//!   отметкой WithdrawLimit (withdraw_limit_pause.rs): «только человек», как Outcome (автоснятия нет — время сброса
//!   лимита биржа не сообщает; новый день и сверка её не снимают). Ставится и поверх паузы без отметки (человек) —
//!   своя отметка «только человек». Пришла поверх Outcome — отметка остаётся
//!   Outcome, а пауза лимита становится «прежней» паузой учёта outcome_open.rs: выяснение исхода вернёт её, а не снимет.
//!
//! Кто вызывает: risk.rs (on_error, resume_venue, set_mode), rules_stop.rs, rules_apply.rs, daily_limit.rs,
//! auto_resume.rs, day_roll.rs, risk_count.rs (no_pause), withdraw_limit_pause.rs, outcome_open.rs, тесты.
//! От чего зависит: risk.rs (Risk, Mode), outcome_open.rs (Open.prior), gateway/errors.rs.
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
    /// Решение 253, дополнение t85: исчерпан лимит вывода аккаунта — снимает только человек (время сброса неизвестно).
    WithdrawLimit,
    /// «Исход неизвестен»: человек или выяснение исхода ботом (решение 231, outcome_open.rs).
    Outcome,
}

/// Вид паузы, которую снимает только человек (не auto_resume.rs, не новый день, не сверка): Outcome, WithdrawLimit.
/// Ошибок нет.
pub fn human_kind(k: Kind) -> bool {
    matches!(k, Kind::Outcome | Kind::WithdrawLimit)
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
/// отметки (сверка, человек) — отметки нет, кроме Outcome: «исход неизвестен» ставит отметку «только
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
        // Дополнение t85: лимит вывода — отметка и поверх паузы без отметки (см. шапку).
        (None, _) if !was || kind == Kind::WithdrawLimit => Some(VenueMark { kind, since: now }),
        (None, _) => None,
    };
    // Дополнение t85: лимит вывода поверх «исход неизвестен» — становится прежней паузой учёта outcome_open.rs.
    if kind == Kind::WithdrawLimit
        && prev.as_ref().is_some_and(|m| m.kind == Kind::Outcome)
        && let Some(o) = r.outcome_open.get_mut(v)
    {
        o.prior = Some((why.clone(), Some(VenueMark { kind, since: now })));
    }
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

/// Пауза площадки снимается только человеком («исход неизвестен», лимит вывода — human_kind). Ошибок нет.
pub fn human_only(r: &Risk, v: &VenueId) -> bool {
    r.venue_auto.get(v).is_some_and(|m| human_kind(m.kind))
}

/// Пора ли проверять подключение для автоснятия. Errors — после ожидания `wait_errors_s` с постановки (не задано —
/// никогда, снимает человек); Rule — на каждом круге; Outcome и WithdrawLimit — никогда.
/// Принимает: отметку, сейчас, ожидание для Errors (секунды). Ошибок нет.
pub fn due(m: &VenueMark, now: DateTime<Utc>, wait_errors_s: Option<u64>) -> bool {
    match m.kind {
        Kind::Errors => wait_errors_s
            .and_then(|w| i64::try_from(w).ok())
            .is_some_and(|w| (now - m.since).num_seconds() >= w),
        Kind::Rule => true,
        Kind::WithdrawLimit | Kind::Outcome => false,
    }
}

/// Снимает софтовую паузу площадки, если отметка та же, что была при проверке (не сменилась и не снята человеком):
/// пауза, счётчик ошибок подряд, отметка. Принимает: состояние риска, площадку, отметку на момент проверки.
/// Возвращает: true — снята. Ошибок нет.
pub fn lift_venue(r: &mut Risk, v: &VenueId, seen: &VenueMark) -> bool {
    let same = r.venue_auto.get(v) == Some(seen) && !human_kind(seen.kind);
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
