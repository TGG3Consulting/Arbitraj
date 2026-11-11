//! Файл: core/daily_commit.rs
//! Что делает: пересчёт итога дня из книги (вынесено из daily.rs): общий итог и разбивка по парам «площадка × тип
//! направления» (решение 258, daily_venue.rs) в Risk.day, запись книги в таблицу daily_result (строка на дату, переживает
//! перезапуск), проверка лимитов: пар (пауза только этого направления этой биржи, daily_venue_limit.rs) и общего (Стоп
//! системы, daily_limit.rs); что сработало (Trip) — в журнал и Telegram (announce).
//! Кто вызывает: daily.rs (record, settle), daily_load.rs (load, transfer), daily_limit.rs и daily_venue_limit.rs
//! (сброс лимита); announce — ledger.rs, ledger_load.rs, daily_load.rs, сбросы лимита.
//! От чего зависит: daily.rs (Book, prov_on), daily_venue.rs, day_pair.rs, daily_venue_limit.rs, daily_limit.rs,
//! deal_text.rs, storage/db.rs.
//! Настройки: нет (лимиты читают daily_venue_limit.rs и daily_limit.rs).

use chrono::{NaiveDate, Utc};
use rusqlite::params;

use crate::storage::journal::Level;

use super::daily::{Book, prov_on};
use super::engine::{Core, lk};

/// Что сработало при пересчёте итога дня: общий «Стоп» (текст) и паузы пар по убытку дня (id пары, причина).
#[derive(Debug, Default)]
pub struct Trip {
    pub stop: Option<String>,
    pub paused: Vec<(String, String)>,
}

/// Что делает: журнал и Telegram по тому, что сработало при пересчёте: общий «Стоп» — core.daily_loss и
/// risk.daily_loss; пауза пары (решение 258) — core.venue_daily_loss и risk.venue_daily_loss с площадкой пары (у
/// «DEX ↔ DEX» — площадка DEX), тексты — решение 164. Принимает: ядро, Trip. Ошибок нет.
pub async fn announce(core: &Core, trip: &Trip) {
    let deps = &core.inner.deps;
    for (p, why) in &trip.paused {
        let v = super::day_pair::venue_or_dex(core, p);
        deps.log(
            Level::Warn,
            "core.venue_daily_loss",
            v.as_deref(),
            why,
            None,
        )
        .await;
        let tg = super::deal_text::pair_daily_stop(p, why);
        deps.notify("risk.venue_daily_loss", v.as_deref(), &tg)
            .await;
    }
    if let Some(stop) = &trip.stop {
        deps.log(Level::Warn, "core.daily_loss", None, stop, None)
            .await;
        let tg = super::deal_text::daily_stop(stop);
        deps.notify("risk.daily_loss", None, &tg).await;
    }
}

/// Пересчитывает DayView из книги дня, сохраняет и проверяет лимиты. Принимает: ядро, день, книгу.
/// Возвращает: что сработало (общий «Стоп», паузы пар) — в журнал и Telegram его отдаёт daily_commit::announce.
pub(super) async fn commit(core: &Core, date: NaiveDate, book: Book) -> Trip {
    let prov = prov_on(&book, date);
    let pairs = super::daily_venue::days(core, &book, date);
    {
        let mut r = lk(&core.inner.risk);
        // Новый день — убыток пар считается с нуля; паузы прошлого дня по убытку сняты в полночь UTC (day_roll.rs,
        // решение 225).
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
        r.day.pairs = pairs;
        r.day.deal_pairs = book.deal_pairs.clone();
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
    let (paused, pair_cleared) = super::daily_venue_limit::check(core);
    let (stop, cleared) = super::daily_limit::check(core);
    // Решение 152: факт пришёл, лимит больше не превышен — остановку снимает человек, но он должен это видеть.
    for t in pair_cleared.into_iter().chain(cleared) {
        deps.log(Level::Info, "core.daily_loss", None, t, None)
            .await;
    }
    Trip { stop, paused }
}
