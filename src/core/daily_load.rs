//! Файл: core/daily_load.rs
//! Что делает: итог дня (вынесено из daily.rs): восстановление при запуске (последняя строка daily_result — факт дня,
//! неразобранные оценки, пометки «цены нет», сброс лимита дня; решение 258 — разбивка по парам «площадка × тип
//! направления» и их сбросы; запись до решения 258 (по площадкам или без разбивки) — разнесение по парам сделок,
//! daily_migrate.rs) и перенос доли оценки при «Вернуть боту».
//! Кто вызывает: engine.rs (load), manual_return.rs (transfer) — через daily.rs.
//! От чего зависит: daily.rs (Book, commit, today_book), daily_migrate.rs, day_pair.rs, daily_commit.rs (announce),
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
    // Решение 258: новая сделка — со своей парой (монеты могли вернуться на другую площадку).
    let pair = super::day_pair::of_direction(core, &to.direction);
    book.deal_pairs.insert(to.id.clone(), pair);
    if book.unpriced.contains(&from.id) {
        book.unpriced.insert(to.id.clone());
    }
    if amount != moved {
        book.provisional
            .insert(from.id.clone(), (amount - moved, day));
    } else {
        book.unpriced.remove(&from.id);
        book.deal_pairs.remove(&from.id);
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
            // Решение 258: запись до разбивки по парам — без поля pair_final (по площадкам или без разбивки).
            let raw = parsed.clone().unwrap_or_default();
            let old = parsed
                .as_ref()
                .is_some_and(|v| v.get("pair_final").is_none());
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
                // Разнесение по парам сделок (сегодня — без двойного счёта; прошлые дни не трогаются).
                super::daily_migrate::split(core, date, today, &raw, &mut book).await;
            }
            let cur = if date == today {
                book
            } else {
                Book {
                    provisional: book.provisional,
                    unpriced: book.unpriced,
                    deal_pairs: book.deal_pairs,
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
