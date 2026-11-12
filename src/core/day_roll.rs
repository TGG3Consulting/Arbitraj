//! Файл: core/day_roll.rs
//! Что делает: решение 225 — новый день (полночь UTC; день учёта — UTC, daily.rs) снимает софтовые паузы по убытку дня:
//! - паузы пар «площадка × тип направления» по их лимиту дня (Risk.pair_paused, решение 258; «dex_dex» — циклы DEX ↔
//!   DEX). Паузы площадки (Risk.venue_paused: ошибки подряд, исход неизвестен, лимит вывода, сверка, человек) — другие
//!   паузы, новый день их не трогает (пауза по убытку с ними больше не смешивается);
//! - «Стоп» системы по общему лимиту дня (Risk.mode_auto вида DailyTotal) — режим до него возвращается; человек менял
//!   режим — отметки нет, ничего не возвращается.
//!   Затем итог дня пересчитывается на новую дату (daily::commit — новый день виден сразу, а не с первой сделкой).
//!   Журнал core.control, уведомление risk.auto_resumed.
//!
//! Кто вызывает: engine.rs (фоновая задача run).
//! От чего зависит: auto_pause.rs, risk.rs, day_pair.rs (подписи), daily.rs (today_book, commit), daily_commit.rs
//! (announce), deps.rs.
//! Настройки: нет (граница дня — полночь UTC, как у учёта дня).

use std::time::Duration;

use chrono::{DateTime, NaiveTime, Utc};

use crate::notify::human::{self as h, Msg};
use crate::storage::journal::Level;

use super::auto_pause::{self as ap, ModeKind};
use super::engine::{Core, lk};
use super::risk::{Mode, Risk};

/// Что сняла смена дня.
#[derive(Debug, Default, PartialEq)]
pub struct DayLift {
    /// Пары «площадка × тип направления» (id), с которых снята пауза по убытку дня (решение 258).
    pub pairs: Vec<String>,
    /// Режим, который вернулся после «Стоп» по общему лимиту дня.
    pub mode: Option<Mode>,
}

/// Снимает паузы по убытку прошедшего дня (чистая логика). Принимает: состояние риска (под блокировкой).
/// Возвращает: что снято. Ошибок нет.
pub fn new_day(r: &mut Risk) -> DayLift {
    let mut out = DayLift {
        pairs: std::mem::take(&mut r.pair_paused).into_keys().collect(),
        mode: None,
    };
    r.loss_seen.clear();
    if let Some(m) = r.mode_auto.clone()
        && m.kind == ModeKind::DailyTotal
    {
        let why =
            "новый день (UTC): «Стоп» по общему лимиту дня снят сам (решение 225)".to_string();
        out.mode = ap::restore_mode(r, &m, why);
    }
    out
}

/// Сколько ждать до ближайшей полуночи UTC. Принимает: сейчас. Возвращает: None — дальше календаря (не бывает).
pub fn until_midnight(now: DateTime<Utc>) -> Option<Duration> {
    let next = now
        .date_naive()
        .succ_opt()?
        .and_time(NaiveTime::MIN)
        .and_utc();
    (next - now).to_std().ok()
}

/// Фоновая задача: ждёт полночь UTC (дата действительно сменилась — часы могли разбудить раньше) и снимает паузы.
/// Работает, пока жива система. Ошибок нет.
pub async fn run(core: Core) {
    loop {
        let Some(target) = Utc::now().date_naive().succ_opt() else {
            return;
        };
        while Utc::now().date_naive() < target {
            let Some(d) = until_midnight(Utc::now()) else {
                return;
            };
            tokio::time::sleep(d).await;
        }
        roll(&core).await;
    }
}

/// Смена дня: снятие пауз, журнал и уведомление, пересчёт итога дня на новую дату. Ошибок нет.
async fn roll(core: &Core) {
    let lift = new_day(&mut lk(&core.inner.risk));
    super::risk::publish(core);
    report(core, &lift).await;
    let (today, book) = super::daily::today_book(core);
    let trip = super::daily::commit(core, today, book).await;
    super::daily_commit::announce(core, &trip).await;
}

/// Журнал core.control (строка на каждое снятие) и одно уведомление risk.auto_resumed. Ничего не снято — тихо.
/// Ошибок нет.
async fn report(core: &Core, l: &DayLift) {
    let deps = &core.inner.deps;
    let mut msg = Msg::new(h::OK, "Новый день — паузы по убытку дня сняты");
    let mut any = false;
    for p in &l.pairs {
        let name = super::day_pair::label(p);
        let t = format!(
            "Новый день (UTC): пауза направления {name} по убытку дня снята сама (решения 225, 258)"
        );
        let v = super::day_pair::venue_or_dex(core, p);
        deps.log(Level::Info, "core.control", v.as_deref(), &t, None)
            .await;
        msg = msg.line(format!("{name} снова торгует"));
        any = true;
    }
    if let Some(m) = l.mode {
        let t = format!(
            "Режим системы: Stopped → {m:?} — новый день (UTC), «Стоп» по общему лимиту дня снят сам (решение 225)"
        );
        deps.log(Level::Info, "core.control", None, &t, None).await;
        msg = msg.kv("Режим", super::commands_control::label(m));
        any = true;
    }
    if any {
        deps.notify("risk.auto_resumed", None, &msg.render()).await;
    }
}
