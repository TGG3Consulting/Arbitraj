//! Файл: core/daily_commit.rs
//! Что делает: пересчёт итога дня из книги (вынесено из daily.rs): общий итог и разбивка по площадкам (решение 173,
//! daily_venue.rs) в Risk.day, запись книги в таблицу daily_result (строка на дату, переживает перезапуск), проверка
//! лимитов: площадок (пауза площадки, daily_venue_limit.rs) и общего (Стоп системы, daily_limit.rs); что сработало
//! (Trip) — в журнал и Telegram (announce).
//! Кто вызывает: daily.rs (record, settle), daily_load.rs (load, transfer), daily_limit.rs и daily_venue_limit.rs
//! (сброс лимита); announce — ledger.rs, ledger_load.rs, daily_load.rs, сбросы лимита.
//! От чего зависит: daily.rs (Book, prov_on), daily_venue.rs, daily_venue_limit.rs, daily_limit.rs, deal_text.rs,
//! storage/db.rs.
//! Настройки: нет (лимиты читают daily_venue_limit.rs и daily_limit.rs).

use chrono::{NaiveDate, Utc};
use rusqlite::params;

use crate::core::types::VenueId;
use crate::storage::journal::Level;

use super::daily::{Book, prov_on};
use super::engine::{Core, lk};

/// Что сработало при пересчёте итога дня: общий «Стоп» (текст) и паузы площадок (id, причина).
#[derive(Debug, Default)]
pub struct Trip {
    pub stop: Option<String>,
    pub paused: Vec<(VenueId, String)>,
}

/// Что делает: журнал и Telegram по тому, что сработало при пересчёте: общий «Стоп» — core.daily_loss и
/// risk.daily_loss; пауза площадки — core.venue_daily_loss и risk.venue_daily_loss (тексты — решение 164).
/// Принимает: ядро, Trip. Ошибок нет.
pub async fn announce(core: &Core, trip: &Trip) {
    let deps = &core.inner.deps;
    for (v, why) in &trip.paused {
        deps.log(Level::Warn, "core.venue_daily_loss", Some(&v.0), why, None)
            .await;
        let tg = super::deal_text::venue_daily_stop(&v.0, why);
        deps.notify("risk.venue_daily_loss", Some(&v.0), &tg).await;
    }
    if let Some(stop) = &trip.stop {
        deps.log(Level::Warn, "core.daily_loss", None, stop, None)
            .await;
        let tg = super::deal_text::daily_stop(stop);
        deps.notify("risk.daily_loss", None, &tg).await;
    }
}

/// Пересчитывает DayView из книги дня, сохраняет и проверяет лимиты. Принимает: ядро, день, книгу.
/// Возвращает: что сработало (общий «Стоп», паузы площадок) — в журнал и Telegram его отдаёт daily_commit::announce.
pub(super) async fn commit(core: &Core, date: NaiveDate, book: Book) -> Trip {
    let prov = prov_on(&book, date);
    let venues = super::daily_venue::days(core, &book, date);
    {
        let mut r = lk(&core.inner.risk);
        // Новый день — убыток площадок считается с нуля; паузы прошлого дня снимает человек.
        if r.day.date != date {
            r.loss_seen.clear();
        }
        r.day.date = date;
        r.day.final_usd = book.final_usd;
        r.day.provisional_usd = prov;
        r.day.result_usd = book.final_usd + prov;
        r.day.provisional = book.provisional.clone();
        r.day.unpriced = book.unpriced.clone();
        r.day.limit_reset = book.limit_reset.clone();
        r.day.venues = venues;
        r.day.deal_venues = book.deal_venues.clone();
    }
    let body = serde_json::to_string(&book).unwrap_or_default();
    let day = date.to_string();
    let res = core
        .inner
        .deps
        .db
        .call(move |c| {
            c.execute(
                "INSERT INTO daily_result (day, body, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(day) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
                params![day, body, Utc::now().to_rfc3339()],
            )
            .map(|_| ())
        })
        .await;
    let deps = &core.inner.deps;
    if let Err(e) = res {
        let m = format!("Итог дня не сохранён: {e}");
        deps.log(Level::Error, "core.daily", None, m, None).await;
    }
    let (paused, venue_cleared) = super::daily_venue_limit::check(core);
    let (stop, cleared) = super::daily_limit::check(core);
    // Решение 152: факт пришёл, лимит больше не превышен — остановку снимает человек, но он должен это видеть.
    for t in venue_cleared.into_iter().chain(cleared) {
        deps.log(Level::Info, "core.daily_loss", None, t, None)
            .await;
    }
    Trip { stop, paused }
}
