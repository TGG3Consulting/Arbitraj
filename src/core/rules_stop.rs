//! Файл: core/rules_stop.rs
//! Что делает: паузы от ошибок операций (вынесено из rules_apply.rs; решение 225 — у каждой паузы «кто поставил»):
//! - «исход неизвестен» (OutcomeUnknown) — пауза площадки с отметкой «только человек» (auto_pause::Kind::Outcome):
//!   сам бот исход такой операции не узнаёт (DECISIONS 225), снимает человек «Снять паузу»; уведомление — один раз на
//!   смену состояния (остановленная очередь кошелька отвечает так на каждую попытку);
//! - правило шлюза «стоп площадка» — пауза площадки с отметкой Kind::Rule (снимет auto_resume.rs проверкой подключения);
//! - правило шлюза «стоп всё» — «Стоп» системы с отметкой ModeKind::Rule (площадка ошибки; снимет auto_resume.rs);
//! - отказ вывода / неверный адрес / вывод заблокирован (auto_pause::no_pause) — «стоп площадка» и «стоп всё» паузу НЕ
//!   ставят: предупреждение в журнал (Telegram — событие exec.withdraw_rejected в rules_apply.rs). Вывод не повторяем,
//!   продаём на месте (решение 220б). Правила в базе не меняются — так трактуется код (решение 225).
//!   Текст уведомления о паузе говорит, снимется ли она сама (resume_note).
//!
//! Кто вызывает: rules_apply.rs.
//! От чего зависит: auto_pause.rs, risk.rs, rules_apply.rs (tg_error), engine.rs, deps.rs, settings.
//! Настройки: risk.auto_resume_check_s, risk.errors_resume_after_s (только текст уведомления).

use chrono::Utc;

use crate::core::types::VenueId;
use crate::gateway::errors::GatewayError;
use crate::notify::human::{self as h, Msg};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::auto_pause::{self as ap, Kind, ModeKind};
use super::engine::{Core, lk};
use super::risk::{self, Mode};

/// «Исход неизвестен»: пауза площадки «только человек»; уведомления send.outcome_unknown и risk.stop — если пауза
/// новая или стала «только человек». Принимает: ядро, операцию, площадку, сделку, ошибку. Ошибок нет.
pub(super) async fn outcome(
    core: &Core,
    op: &str,
    venue: &VenueId,
    deal: Option<&str>,
    err: &GatewayError,
) {
    let why = format!("{op}: исход транзакции неизвестен — нужна ручная проверка");
    let fresh = ap::pause_venue(
        &mut lk(&core.inner.risk),
        venue,
        why.clone(),
        Kind::Outcome,
        Utc::now(),
    );
    if !fresh {
        return;
    }
    risk::publish(core);
    let deps = &core.inner.deps;
    let tg = super::rules_apply::tg_error("send.outcome_unknown", op, venue, deal, err)
        .todo(format!(
            "проверить на {}, прошла ли операция, затем «Снять паузу» площадки",
            h::venue(&venue.0)
        ))
        .render();
    deps.notify("send.outcome_unknown", Some(&venue.0), &tg)
        .await;
    deps.notify("risk.stop", Some(&venue.0), &paused_text(venue, &why, None))
        .await;
}

/// Правило «стоп площадка»: пауза площадки (Kind::Rule), уведомление risk.stop; код из no_pause — только журнал.
/// Принимает: ядро, № правила, площадку, ошибку. Ошибок нет.
pub(super) async fn stop_venue(core: &Core, rule: i64, venue: &VenueId, err: &GatewayError) {
    if no_pause_log(core, rule, "стоп площадка", venue, err).await {
        return;
    }
    let why = format!("правило шлюза №{rule}: {:?}", err.code);
    risk::on_error(core, venue);
    ap::pause_venue(
        &mut lk(&core.inner.risk),
        venue,
        why.clone(),
        Kind::Rule,
        Utc::now(),
    );
    risk::publish(core);
    let note = resume_note(core, venue, Kind::Rule);
    core.inner
        .deps
        .notify("risk.stop", Some(&venue.0), &paused_text(venue, &why, note))
        .await;
}

/// Правило «стоп всё»: «Стоп» системы с отметкой ModeKind::Rule (режим до него запоминается), журнал core.control,
/// уведомление risk.stop; код из no_pause — только журнал. Принимает: ядро, № правила, операцию, площадку, сделку,
/// ошибку. Ошибок нет.
pub(super) async fn stop_all(
    core: &Core,
    rule: i64,
    op: &str,
    venue: &VenueId,
    deal: Option<&str>,
    err: &GatewayError,
) {
    if no_pause_log(core, rule, "стоп всё", venue, err).await {
        return;
    }
    let why = format!("правило шлюза №{rule}: {:?} на {}", err.code, venue.0);
    let (prev, prev_reason) = {
        let r = lk(&core.inner.risk);
        (r.mode, r.reason.clone())
    };
    let log = risk::set_mode(core, Mode::Stopped, Some(why.clone()));
    let kind = ModeKind::Rule {
        venue: venue.clone(),
    };
    ap::mark_mode(
        &mut lk(&core.inner.risk),
        kind,
        prev,
        prev_reason,
        Utc::now(),
    );
    risk::publish(core);
    let deps = &core.inner.deps;
    deps.log(Level::Info, "core.control", Some(&venue.0), &log, None)
        .await;
    let note = (prev != Mode::Stopped)
        .then(|| resume_note(core, venue, Kind::Rule))
        .flatten();
    let tg = Msg::new(h::STOP, "Торговля остановлена")
        .line(format!("Ошибка на {}: {op}", h::venue(&venue.0)))
        .reason(&err.message)
        .kv("Сработало", format!("правило шлюза №{rule}"))
        .line(note.unwrap_or_default())
        .id(deal.unwrap_or_default())
        .render();
    deps.notify("risk.stop", Some(&venue.0), &tg).await;
}

/// Код без паузы (auto_pause::no_pause): запись в журнал, что правило паузу не поставило. Принимает: ядро, № правила,
/// название действия, площадку, ошибку. Возвращает: true — паузы нет (вызывающий выходит). Ошибок нет.
async fn no_pause_log(
    core: &Core,
    rule: i64,
    what: &str,
    venue: &VenueId,
    err: &GatewayError,
) -> bool {
    if !ap::no_pause(err.code) {
        return false;
    }
    core.inner
        .deps
        .log(
            Level::Warn,
            "core.rules",
            Some(&venue.0),
            format!(
                "Правило шлюза №{rule} ({:?}) — «{what}»: паузы нет — вывод не повторяем, монета продаётся на месте \
                 (решения 220б, 225)",
                err.code
            ),
            None,
        )
        .await;
    true
}

/// Снимется ли софтовая пауза сама — строка для уведомления. Принимает: ядро, площадку, вид паузы.
/// Возвращает: None — нужные настройки не заданы (паузу снимает человек) или вид Outcome. Ошибок нет.
pub(super) fn resume_note(core: &Core, venue: &VenueId, kind: Kind) -> Option<String> {
    let s = &core.inner.deps.settings;
    let at = At {
        venue: Some(&venue.0),
        coin: None,
    };
    let every = s.integer("risk.auto_resume_check_s", At::default()).ok()?;
    match kind {
        Kind::Errors => {
            let wait = s.integer("risk.errors_resume_after_s", at).ok()?;
            Some(format!(
                "Снимется сама: через {wait} с бот проверит подключение площадки (дальше — каждые {every} с), \
                 «Подключено» — пауза снята"
            ))
        }
        Kind::Rule => Some(format!(
            "Снимется сама: бот проверяет подключение площадки каждые {every} с, «Подключено» — пауза снята"
        )),
        Kind::Outcome => None,
    }
}

/// Решение 164: площадка на паузе (risk.stop). Принимает: площадку, причину паузы, строку «снимется сама» (None —
/// снимает человек). Ошибок нет.
pub(super) fn paused_text(venue: &VenueId, why: &str, note: Option<String>) -> String {
    let todo = if note.is_some() {
        "или разобраться с причиной и «Снять паузу» на экране «Управление»"
    } else {
        "разобраться с причиной, затем «Снять паузу» на экране «Управление»"
    };
    Msg::new(h::STOP, format!("Площадка {} на паузе", h::venue(&venue.0)))
        .reason(why)
        .line(note.unwrap_or_default())
        .todo(todo)
        .render()
}
