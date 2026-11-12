//! Файл: core/auto_resume.rs
//! Что делает: решение 225 — бот сам снимает паузы, которые сам поставил, когда причина ушла: раз в
//! risk.auto_resume_check_s проверяет подключение площадки (то же, что кнопка «Проверить подключение»: шлюз
//! check_connection — ключ, права, запрос баланса/аккаунта; у DEX — узел и кошелёк) и при «Подключено» снимает:
//! - паузу площадки по ошибкам подряд (auto_pause::Kind::Errors) — не раньше risk.errors_resume_after_s после постановки;
//! - паузу площадки по правилу шлюза «стоп площадка» (Kind::Rule) — на каждом круге;
//! - «Стоп» системы по правилу «стоп всё» (ModeKind::Rule) — проверяется площадка, чья ошибка вызвала стоп; режим до
//!   стопа возвращается.
//!   Не трогает: паузы без отметки (человек, другой модуль), «исход неизвестен» (только человек), паузы пар по убытку
//!   дня (Risk.pair_paused — снимет day_roll.rs). Отметка сменилась за время проверки (человек снял / поставили сильнее)
//!   — ничего не делает. Решение 258: пауза по убытку дня — по паре и отдельно от паузы площадки (её ставит проверка
//!   лимитов независимо от пауз площадки), поэтому снятие паузы площадки направления с превышенным лимитом не открывает:
//!   их пауза остаётся, остальные направления площадки торгуют (прежнее превращение паузы площадки в паузу по убытку
//!   убрано). Общий лимит превышен — «Стоп» становится «Стоп» по лимиту дня.
//!   Журнал core.control (снято) и core.auto_resume (проверка не прошла — один раз на смену причины), уведомление
//!   risk.auto_resumed. Не задана risk.auto_resume_check_s — автоснятия нет (одна запись core.not_configured).
//!
//! Кто вызывает: engine.rs (фоновая задача run).
//! От чего зависит: auto_pause.rs, risk.rs, wait.rs (Pacer), gateway (registry, check_connection), daily_limit.rs,
//! deal_text.rs, deps.rs.
//! Настройки: risk.auto_resume_check_s, risk.errors_resume_after_s (вся система, можно по площадке).

use std::collections::HashMap;

use chrono::Utc;
use tokio::sync::Notify;

use crate::core::types::VenueId;
use crate::notify::human::{self as h, Msg};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::auto_pause::{self as ap, ModeKind, ModeMark, VenueMark};
use super::engine::{Core, lk};
use super::wait::{Pacer, Unit};

/// Фоновая задача автоснятия. Работает, пока жива система. Ошибок нет.
pub async fn run(core: Core) {
    let mut pacer = Pacer::new("risk.auto_resume_check_s", Unit::Seconds, None);
    let quiet = Notify::new();
    let mut last: HashMap<VenueId, String> = HashMap::new();
    loop {
        pacer.pause(&core.inner.deps, &quiet).await;
        round(&core, &mut last).await;
    }
}

/// Один круг: какие софтовые паузы пора проверять, проверка подключения (каждая площадка — один раз), снятие.
/// Принимает: ядро, последние причины неудачных проверок. Ошибок нет.
async fn round(core: &Core, last: &mut HashMap<VenueId, String>) {
    let now = Utc::now();
    let (marks, mode) = {
        let r = lk(&core.inner.risk);
        (r.venue_auto.clone(), r.mode_auto.clone())
    };
    let s = &core.inner.deps.settings;
    let due: Vec<(VenueId, VenueMark)> = marks
        .into_iter()
        .filter(|(v, m)| {
            let at = At {
                venue: Some(&v.0),
                coin: None,
            };
            ap::due(m, now, s.integer("risk.errors_resume_after_s", at).ok())
        })
        .collect();
    let mode = mode.filter(|m| matches!(m.kind, ModeKind::Rule { .. }));
    let mut checked: HashMap<VenueId, Result<String, String>> = HashMap::new();
    let mode_venue = mode.as_ref().and_then(|m| match &m.kind {
        ModeKind::Rule { venue } => Some(venue.clone()),
        ModeKind::DailyTotal => None,
    });
    for v in due.iter().map(|(v, _)| v.clone()).chain(mode_venue.clone()) {
        if let std::collections::hash_map::Entry::Vacant(e) = checked.entry(v) {
            let res = check(core, e.key()).await;
            e.insert(res);
        }
    }
    for (v, m) in &due {
        match &checked[v] {
            Ok(d) => venue(core, v, m, d).await,
            Err(e) => failed(core, v, e, last).await,
        }
    }
    if let (Some(m), Some(v)) = (&mode, &mode_venue) {
        match &checked[v] {
            Ok(d) => system(core, m, v, d).await,
            Err(e) => failed(core, v, e, last).await,
        }
    }
    last.retain(|v, _| checked.contains_key(v));
}

/// Проверка подключения площадки — как кнопка «Проверить подключение» (api/check.rs venue). Принимает: ядро, площадку.
/// Возвращает: Ok(подробности «Подключено») или Err(точная причина). Ошибок нет.
async fn check(core: &Core, v: &VenueId) -> Result<String, String> {
    let x = core.inner.deps.gw.get(v).await.map_err(|e| e.message)?;
    match x.check_connection().await {
        Ok(c) if c.ok => Ok(c.details),
        Ok(c) => Err(c.details),
        Err(e) => Err(e.message),
    }
}

/// Проверка не прошла: запись core.auto_resume, если причина новая. Ошибок нет.
async fn failed(core: &Core, v: &VenueId, e: &str, last: &mut HashMap<VenueId, String>) {
    if last.get(v).map(String::as_str) == Some(e) {
        return;
    }
    last.insert(v.clone(), e.to_string());
    let t = format!(
        "Автоснятие паузы {}: проверка подключения не прошла — {e}; пауза остаётся, следующая проверка через \
         risk.auto_resume_check_s (решение 225)",
        h::venue(&v.0)
    );
    core.inner
        .deps
        .log(Level::Info, "core.auto_resume", Some(&v.0), &t, None)
        .await;
}

/// «Подключено» — снять паузу площадки. Паузы её пар по убытку дня (Risk.pair_paused) не трогает (решение 258).
/// Принимает: ядро, площадку, отметку на момент проверки, подробности проверки. Ошибок нет.
async fn venue(core: &Core, v: &VenueId, m: &VenueMark, details: &str) {
    let (why, done) = {
        let mut r = lk(&core.inner.risk);
        let why = r.venue_paused.get(v).cloned().unwrap_or_default();
        if r.venue_auto.get(v) != Some(m) {
            return;
        }
        (why, ap::lift_venue(&mut r, v, m))
    };
    super::risk::publish(core);
    if !done {
        return;
    }
    let deps = &core.inner.deps;
    let t = format!(
        "Пауза площадки {} снята автоматически: её поставил бот ({:?}: {why}), проверка подключения — Подключено \
         ({details}) (решение 225)",
        h::venue(&v.0),
        m.kind
    );
    deps.log(Level::Info, "core.control", Some(&v.0), &t, None)
        .await;
    let tg = Msg::new(h::OK, format!("{} снова торгует", h::venue(&v.0)))
        .kv("Была пауза", why)
        .line("Бот сам проверил подключение — «Подключено», пауза снята")
        .render();
    deps.notify("risk.auto_resumed", Some(&v.0), &tg).await;
}

/// «Подключено» у площадки «стоп всё» — вернуть режим до стопа (общий лимит дня превышен — «Стоп» становится «Стоп»
/// по лимиту дня, снимется с новым днём). Принимает: ядро, отметку на момент проверки, площадку, подробности. Ошибок нет.
async fn system(core: &Core, m: &ModeMark, v: &VenueId, details: &str) {
    let day = lk(&core.inner.risk).day.clone();
    let over = super::daily_limit::view(core, &day).filter(|l| l.exceeded);
    let why = format!(
        "правило «стоп всё» снято само: {} — Подключено (решение 225)",
        h::venue(&v.0)
    );
    let back = {
        let mut r = lk(&core.inner.risk);
        match &over {
            Some(l) if r.mode_auto.as_ref() == Some(m) => {
                let t = format!(
                    "вся система: убыток за день {} больше общего лимита {}",
                    h::usd(l.loss_usd),
                    h::usd(l.max_usd)
                );
                if let Some(x) = r.mode_auto.as_mut() {
                    x.kind = ModeKind::DailyTotal;
                }
                r.daily_stop = true;
                r.reason = Some(t.clone());
                Err(t)
            }
            Some(_) => return,
            None => ap::restore_mode(&mut r, m, why).ok_or_else(String::new),
        }
    };
    super::risk::publish(core);
    let deps = &core.inner.deps;
    match back {
        Ok(mode) => {
            let t = format!(
                "Режим системы: Stopped → {mode:?} — «Стоп» по правилу шлюза снят сам: бот проверил подключение {} — \
                 Подключено ({details}) (решение 225)",
                h::venue(&v.0)
            );
            deps.log(Level::Info, "core.control", None, &t, None).await;
            let tg = Msg::new(h::OK, "Торговля снова идёт")
                .line(format!(
                    "Бот сам проверил подключение {} — «Подключено», «Стоп» снят",
                    h::venue(&v.0)
                ))
                .kv("Режим", super::commands_control::label(mode))
                .render();
            deps.notify("risk.auto_resumed", None, &tg).await;
        }
        Err(t) if !t.is_empty() => {
            let j = format!(
                "{t} — «Стоп» по правилу шлюза не снят, а стал «Стоп» по лимиту дня: снимется с новым днём (UTC)"
            );
            deps.log(Level::Warn, "core.daily_loss", None, &j, None)
                .await;
            deps.notify("risk.daily_loss", None, &super::deal_text::daily_stop(&t))
                .await;
        }
        Err(_) => {}
    }
}
