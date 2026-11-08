//! Файл: core/daily_venue_limit.rs
//! Что делает: лимит убытка за день по площадке (решение 173). Лимит — risk.max_daily_loss_usd: значение площадки, а
//! если его нет — значение «вся система» (лимит по умолчанию для каждой площадки). Убыток площадки =
//! −(её итог дня − её итог в момент сброса); без сброса — −итог (итог — daily_venue.rs). Достигнут — ПАУЗА ТОЛЬКО этой
//! площадки (тот же Risk.venue_paused, что у ошибок подряд: новые сделки с её участием не начинаются, идущие
//! доводятся); остальные торгуют. Причина паузы: «MEXC: убыток за день X больше лимита Y». У DEX убыток копят только
//! циклы DEX ↔ DEX — пауза только циклов (Risk.cycle_paused, «DEX ↔ DEX: убыток за день X больше лимита Y»), а не
//! площадки DEX целиком: направления BingX ↔ DEX и другие CEX ↔ DEX торгуют.
//! Повторная пауза: после «Снять паузу» без сброса площадка торгует, пока её убыток не вырастет — новый минус при
//! превышенном лимите снова ставит паузу; плюсовая сделка или чужая площадка паузу не возвращают.
//! Снятие: «Снять паузу» площадки (risk.rs) или «Сбросить лимит дня» по ней (reset; общий сброс — daily_limit.rs).
//! Решение 221: площадка на паузе от сверки (Risk.reconcile_venues) — пауза по убытку ставится поверх и снимает отметку
//! сверки (такая пауза сама не снимается).
//! Кто вызывает: daily_commit.rs (check), daily_limit.rs (общий сброс), risk.rs (snapshot), commands_control.rs, api.
//! От чего зависит: daily.rs (today_book, commit), daily_venue.rs, daily_limit.rs (describe), daily_commit.rs (announce),
//! day_view.rs.
//! Настройки: risk.max_daily_loss_usd (вся система, можно по площадке).

use chrono::{NaiveDate, Utc};

use crate::core::types::VenueId;
use crate::notify::human::{INFO, Msg, venue as name};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::day_view::{LimitReset, LimitView, VenueDay};
use super::engine::{Core, lk};

/// Что делает: лимит площадки по её итогу дня и настройке. Принимает: ядро, id, итог площадки, день.
/// Возвращает: None — risk.max_daily_loss_usd не задана ни для площадки, ни для всей системы. Ошибок нет.
pub fn view(core: &Core, venue: &str, x: &VenueDay, date: NaiveDate) -> Option<LimitView> {
    let at = At {
        venue: Some(venue),
        coin: None,
    };
    let max = core
        .inner
        .deps
        .settings
        .decimal("risk.max_daily_loss_usd", at)
        .ok()?;
    Some(LimitView::of(
        max,
        x.limit_reset.as_ref(),
        date,
        x.result_usd,
    ))
}

/// Текст лимита: «MEXC: убыток за день 20 USDT больше лимита 15 USDT (…)», у DEX — «DEX ↔ DEX: …». Ошибок нет.
fn describe(core: &Core, venue: &str, x: &VenueDay, l: &LimitView, rel: &str) -> String {
    let t = super::daily_limit::describe(x.limit_reset.as_ref(), x.provisional_usd, l, rel);
    format!("{}: {t}", super::daily_venue::label(core, venue))
}

/// Что делает: лимиты площадок после пересчёта итога дня; достигнут — пауза площадки.
/// Возвращает: (площадки, поставленные на паузу сейчас, с причиной; тексты «лимит площадки сейчас не превышен»).
pub(super) fn check(core: &Core) -> (Vec<(VenueId, String)>, Vec<String>) {
    let mut paused = Vec::new();
    let mut cleared = Vec::new();
    let mut r = lk(&core.inner.risk);
    let date = r.day.date;
    let names: Vec<String> = r.day.venues.keys().cloned().collect();
    for v in names {
        let x = r.day.venues[&v].clone();
        let l = view(core, &v, &x, date);
        let id = VenueId(v.clone());
        // DEX: пауза только циклов DEX ↔ DEX (Risk.cycle_paused), направления CEX ↔ DEX торгуют.
        let dex = super::daily_venue::is_dex(core, &v);
        let on = r.loss_paused.contains(&v);
        let seen = r.loss_seen.get(&v).copied();
        match &l {
            Some(l) => {
                r.loss_seen.insert(v.clone(), l.loss_usd);
                // Пауза — при переходе через лимит или при росте убытка (после «Снять паузу» без сброса).
                let grew = seen.is_none_or(|p| p < l.max_usd || l.loss_usd > p);
                // Решение 221: пауза площадки от сверки не мешает паузе по убытку — она её заменяет (сама не снимается).
                let busy = if dex {
                    r.cycle_paused.contains_key(&v)
                } else {
                    r.venue_paused.contains_key(&id) && !r.reconcile_venues.contains(&id)
                };
                if l.exceeded && grew && !on && !busy {
                    let t = describe(core, &v, &x, l, "больше лимита");
                    if dex {
                        r.cycle_paused.insert(v.clone(), t.clone());
                    } else {
                        r.venue_paused.insert(id.clone(), t.clone());
                        super::risk::forget_reconcile(&mut r, &id);
                    }
                    r.loss_paused.insert(v.clone());
                    paused.push((id, t));
                } else if !l.exceeded && on && seen.is_some_and(|p| p >= l.max_usd) {
                    let t = super::daily_limit::describe(
                        x.limit_reset.as_ref(),
                        x.provisional_usd,
                        l,
                        "из лимита",
                    );
                    cleared.push(format!(
                        "{}: лимит дня сейчас не превышен: {t}. Пауза площадки сама не снимается — «Снять паузу» \
                         или «Сбросить лимит дня» по площадке",
                        super::daily_venue::label(core, &v)
                    ));
                }
            }
            None => {
                r.loss_seen.remove(&v);
            }
        }
        let paused_now = r.loss_paused.contains(&v);
        if let Some(e) = r.day.venues.get_mut(&v) {
            e.limit = l;
            e.paused = paused_now;
        }
    }
    (paused, cleared)
}

/// Что делает: «Сбросить лимит дня» по площадке: отметка времени и её итога; пауза по убытку дня снимается; журнал
/// core.loss_limit_reset, уведомление risk.daily_limit_reset. Итог дня, история и сделки не меняются.
/// Принимает: ядро, id площадки, кто нажал. Возвращает: текст для журнала. Ошибок нет.
pub async fn reset(core: &Core, venue: &str, who: &str) -> String {
    let (today, mut book) = super::daily::today_book(core);
    let x = super::daily_venue::days(core, &book, today)
        .remove(venue)
        .unwrap_or_default();
    let at = Utc::now();
    book.venue_resets.insert(
        venue.to_string(),
        LimitReset {
            at,
            base_usd: x.result_usd,
            by: who.to_string(),
        },
    );
    let lifted = lift(core, venue);
    let trip = super::daily::commit(core, today, book).await;
    let v = name(venue);
    let text = format!(
        "Лимит дня площадки {v} сброшен ({who}) в {} UTC: её итог дня в этот момент {} USDT. Дальше лимит площадки \
         считает только изменения после отметки; итог дня, история и сделки не меняются.{}",
        at.format("%H:%M:%S"),
        x.result_usd.round_dp(4),
        if lifted {
            " Пауза площадки по убытку дня снята — она снова торгует"
        } else {
            ""
        }
    );
    let tg = Msg::new(INFO, format!("Лимит дня {v} сброшен"))
        .kv("Кто", who)
        .line(format!(
            "Дальше лимит {v} считает только убытки после {} UTC; итог дня и сделки не меняются",
            at.format("%H:%M")
        ))
        .line(if lifted {
            format!("Пауза {v} снята — торговля на ней идёт")
        } else {
            String::new()
        })
        .render();
    let deps = &core.inner.deps;
    deps.log(
        Level::Info,
        "core.loss_limit_reset",
        Some(venue),
        &text,
        None,
    )
    .await;
    deps.notify("risk.daily_limit_reset", Some(venue), &tg)
        .await;
    super::daily_commit::announce(core, &trip).await;
    text
}

/// Снимает паузу площадки по убытку дня (если она была именно по убытку) и забывает замеченный убыток.
/// Принимает: ядро, id. Возвращает: была ли снята пауза. Ошибок нет.
pub(super) fn lift(core: &Core, venue: &str) -> bool {
    let mut r = lk(&core.inner.risk);
    r.loss_seen.remove(venue);
    let on = r.loss_paused.remove(venue);
    if on {
        let id = VenueId(venue.to_string());
        super::risk::forget_reconcile(&mut r, &id);
        r.venue_paused.remove(&id);
        r.cycle_paused.remove(venue);
    }
    on
}

impl Core {
    /// «Сбросить лимит дня» площадки (интерфейс, Telegram; решение 173). Возвращает: текст для журнала.
    pub async fn reset_venue_daily_limit(&self, venue: &str, who: &str) -> String {
        reset(self, venue, who).await
    }
}
