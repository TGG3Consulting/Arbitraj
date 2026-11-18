//! Файл: core/daily_commit.rs
//! Что делает: пересчёт итога дня из книги (вынесено из daily.rs): общий итог и разбивка по парам «площадка × тип
//! направления» (решение 258, daily_venue.rs) в Risk.day, запись книги в таблицу daily_result (строка на дату, переживает
//! перезапуск), проверка лимитов: пар (пауза только этого направления этой биржи, daily_venue_limit.rs) и общего (Стоп
//! системы, daily_limit.rs); что сработало (Trip) — в журнал и Telegram (announce).
//! Кто вызывает: daily.rs (record, settle), daily_load.rs (load, transfer), daily_limit.rs и daily_venue_limit.rs
//! (сброс лимита); announce — ledger.rs, ledger_load.rs, daily_load.rs, сбросы лимита.
//! От чего зависит: daily.rs (Book, prov_on), daily_venue.rs, day_pair.rs, daily_venue_limit.rs, daily_limit.rs,
//! deal_text.rs, storage/db.rs (решение 353: таблица daily_result — в файле учёта ledger.sqlite, Db::ledger).
//! Решение 414: перед записью — просадка пар от пика (drawdown_limit.rs advance: теоретический баланс и пик в книге),
//! после — её проверка (drawdown_limit.rs check: пауза пары по просадке, Trip.draw).
//! Настройки: нет (лимиты читают daily_venue_limit.rs, drawdown_limit.rs и daily_limit.rs).

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
    /// Решение 414: паузы пар по просадке от пика (id пары, причина с цифрами — drawdown_limit.rs).
    pub draw: Vec<(String, String)>,
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
        // Решение 408: «Снять паузу направления» и «Сбросить лимит дня» направления на тревоге.
        let acts = vec![
            crate::notify::acts::resume_venue(p),
            crate::notify::acts::reset_venue_limit(p),
        ];
        deps.notify_acts("risk.venue_daily_loss", v.as_deref(), &tg, acts)
            .await;
    }
    // Решение 414: пауза направления по просадке от пика — то же событие risk.venue_daily_loss, свой текст с цифрами.
    for (p, why) in &trip.draw {
        let v = super::day_pair::venue_or_dex(core, p);
        deps.log(
            Level::Warn,
            "core.venue_daily_loss",
            v.as_deref(),
            why,
            None,
        )
        .await;
        let tg = super::drawdown_limit::stop_text(p, why);
        let acts = vec![
            crate::notify::acts::resume_venue(p),
            crate::notify::acts::reset_venue_limit(p),
        ];
        deps.notify_acts("risk.venue_daily_loss", v.as_deref(), &tg, acts)
            .await;
    }
    if let Some(stop) = &trip.stop {
        deps.log(Level::Warn, "core.daily_loss", None, stop, None)
            .await;
        let tg = super::deal_text::daily_stop(stop);
        // Решение 408: «Сбросить лимит дня» и «Старт» на тревоге.
        let acts = vec![
            crate::notify::acts::reset_daily(),
            crate::notify::acts::start(),
        ];
        deps.notify_acts("risk.daily_loss", None, &tg, acts).await;
    }
}

/// Пересчитывает DayView из книги дня, сохраняет и проверяет лимиты. Принимает: ядро, день, книгу.
/// Возвращает: что сработало (общий «Стоп», паузы пар) — в журнал и Telegram его отдаёт daily_commit::announce.
pub(super) async fn commit(core: &Core, date: NaiveDate, mut book: Book) -> Trip {
    let prov = prov_on(&book, date);
    let pairs = super::daily_venue::days(core, &book, date);
    // Решение 414: изменения итога пар — в теоретический баланс просадки (пик поднимается); плановые затраты возврата
    // USDT — по сделкам учёта, у сделки этого пересчёта — её собственные (ревью t199, С1: drawdown_pending.rs).
    let pending = super::drawdown_pending::pending(core, book.cur.as_ref());
    super::drawdown_limit::advance(&mut book, &pairs, date, &pending);
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
        r.day.draw = book.draw.clone();
        r.day.draw_want = book.draw_want.clone();
        r.day.draw_pending = pending;
        let want = r.day.draw_want.clone();
        for (p, x) in r.day.pairs.iter_mut() {
            x.draw_want = want.get(p).cloned();
        }
    }
    // Решение 414 (ревью t199, С4 и мелкое 6): просадка проверяется раньше старого лимита дня и до записи — паузы по
    // просадке и замеченная просадка пишутся в ту же строку учёта (переживают перезапуск).
    let (draw, draw_cleared) = super::drawdown_limit::check(core);
    {
        let r = lk(&core.inner.risk);
        book.draw_paused = r
            .draw_paused
            .iter()
            .map(|p| (p.clone(), r.pair_paused.get(p).cloned().unwrap_or_default()))
            .collect();
        book.draw_seen = r.draw_seen.clone();
    }
    let body = serde_json::to_string(&book).unwrap_or_default();
    let day = date.to_string();
    // Будущее записи — в куче (Box::pin): будущее закрытия сделки не растёт на его размер.
    let res = Box::pin(core.inner.deps.db.ledger().write(move |c| {
            c.execute(
                "INSERT INTO daily_result (day, body, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(day) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
                params![day, body, Utc::now().to_rfc3339()],
            )
            .map(|_| ())
        }))
        .await;
    let deps = &core.inner.deps;
    if let Err(e) = res {
        let m = format!("Итог дня не сохранён: {e}");
        deps.log(Level::Error, "core.daily", None, m, None).await;
    }
    let (paused, pair_cleared) = super::daily_venue_limit::check(core);
    let (stop, cleared) = super::daily_limit::check(core);
    // Решение 152: факт пришёл, лимит больше не превышен — остановку снимает человек, но он должен это видеть.
    for t in pair_cleared.into_iter().chain(draw_cleared).chain(cleared) {
        deps.log(Level::Info, "core.daily_loss", None, t, None)
            .await;
    }
    Trip { stop, paused, draw }
}
