//! Файл: core/daily_load.rs
//! Что делает: итог дня (вынесено из daily.rs): восстановление при запуске (последняя строка daily_result — факт дня,
//! неразобранные оценки, пометки «цены нет», сброс лимита дня; решение 173 — разбивка по площадкам и их сбросы;
//! запись без разбивки — разнесение по площадкам сделок, daily_migrate.rs; t91 — запись по прежнему правилу «биржа ↔
//! биржа — обеим биржам» приводится к «бирже покупки», daily_rule.rs) и перенос доли оценки при «Вернуть боту».
//! Кто вызывает: engine.rs (load), manual_return.rs (transfer) — через daily.rs.
//! От чего зависит: daily.rs (Book, commit, today_book), daily_migrate.rs, daily_rule.rs, daily_venue.rs, daily_commit.rs (announce),
//! deal.rs, storage/db.rs.
//! Настройки: нет.

use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;

use crate::storage::journal::Level;

use super::daily::{Book, commit, today_book};
use super::deal::Deal;
use super::engine::Core;

/// «Вернуть боту»: доля оценки переходит к новой сделке (по доле вернувшихся монет), остаток остаётся у старой
/// (монеты, которых нет на балансе, — потеряны). День учёта и пометка «цены нет» сохраняются. Ошибок нет.
pub async fn transfer(core: &Core, from: &mut Deal, to: &mut Deal, share: Decimal) {
    let (today, mut book) = today_book(core);
    let Some((amount, day)) = book.provisional.remove(&from.id) else {
        return;
    };
    let moved = amount * share;
    // Решение 257: доля оценки переходит — начисления «−доля» у старой и «+доля» у новой в день учёта оценки.
    super::deal_book::day(
        from,
        day,
        super::deal_book::DayPart::Estimate,
        -moved,
        "доля оценки ушла «Вернуть боту»",
    );
    super::deal_book::day(
        to,
        day,
        super::deal_book::DayPart::Estimate,
        moved,
        "доля оценки от «Беру на себя»",
    );
    book.provisional.insert(to.id.clone(), (moved, day));
    // Решение 173: новая сделка — со своими площадками (монеты могли вернуться на другую площадку).
    let venues = super::daily_venue::of_direction(core, &to.direction);
    book.deal_venues.insert(to.id.clone(), venues);
    if book.unpriced.contains(&from.id) {
        book.unpriced.insert(to.id.clone());
    }
    if amount != moved {
        book.provisional
            .insert(from.id.clone(), (amount - moved, day));
    } else {
        book.unpriced.remove(&from.id);
        book.deal_venues.remove(&from.id);
    }
    from.provisional_usd = Some(amount - moved);
    to.provisional_usd = Some(moved);
    let trip = commit(core, today, book).await;
    super::daily_commit::announce(core, &trip).await;
}

/// Восстанавливает итог дня при запуске: последняя строка daily_result. Ошибка базы — в журнал.
pub async fn load(core: &Core) {
    let rows = core
        .inner
        .deps
        .db
        .call(|c| {
            c.query_row(
                "SELECT day, body FROM daily_result ORDER BY day DESC LIMIT 1",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .map(Some)
            .or_else(|e| {
                if e == rusqlite::Error::QueryReturnedNoRows {
                    Ok(None)
                } else {
                    Err(e)
                }
            })
        })
        .await;
    match rows {
        Ok(Some((day, body))) => {
            let parsed = serde_json::from_str::<serde_json::Value>(&body).ok();
            // Решение 173: запись до разбивки по площадкам — без поля venue_final.
            let old = parsed
                .as_ref()
                .is_some_and(|v| v.get("venue_final").is_none());
            // t91: правило площадок записи (нет поля — решение 173, биржа ↔ биржа — обеим биржам).
            let rule = parsed
                .as_ref()
                .and_then(|v| v.get("venue_rule"))
                .and_then(|r| r.as_u64())
                .unwrap_or(0);
            let (Ok(date), Some(Ok(mut book))) = (
                day.parse::<NaiveDate>(),
                parsed.map(serde_json::from_value::<Book>),
            ) else {
                core.inner
                    .deps
                    .log(
                        Level::Error,
                        "core.daily",
                        None,
                        format!("Итог дня {day} повреждён"),
                        None,
                    )
                    .await;
                return;
            };
            // Другой день — факт и сбросы того дня в сегодняшний итог не входят; неразобранные оценки переносятся.
            let today = Utc::now().date_naive();
            if old {
                // Разнесение по сделкам — сразу по нынешнему правилу (daily_venue::of_direction).
                super::daily_migrate::split(core, date, &mut book).await;
            } else if rule < u64::from(super::daily_venue::RULE) {
                // t91: запись по прежнему правилу — к новому (сегодня — без двойного счёта; прошлые дни не трогаются).
                super::daily_rule::fix(core, date, today, &mut book).await;
            }
            let cur = if date == today {
                book
            } else {
                Book {
                    provisional: book.provisional,
                    unpriced: book.unpriced,
                    deal_venues: book.deal_venues,
                    ..Book::default()
                }
            };
            let trip = commit(core, today, cur).await;
            super::daily_commit::announce(core, &trip).await;
        }
        Ok(None) => {}
        Err(e) => {
            core.inner
                .deps
                .log(
                    Level::Error,
                    "core.daily",
                    None,
                    format!("Итог дня не загружен: {e}"),
                    None,
                )
                .await
        }
    }
}
