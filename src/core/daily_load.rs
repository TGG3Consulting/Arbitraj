//! Файл: core/daily_load.rs
//! Что делает: итог дня (вынесено из daily.rs): восстановление при запуске (последняя строка daily_result — факт дня,
//! неразобранные оценки, пометки «цены нет», сброс лимита дня) и перенос доли оценки при «Вернуть боту».
//! Кто вызывает: engine.rs (load), manual_return.rs (transfer) — через daily.rs.
//! От чего зависит: daily.rs (Book, commit, today_book), deal.rs, storage/db.rs.
//! Настройки: нет.

use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::storage::journal::Level;

use super::daily::{Book, commit, today_book};
use super::deal::Deal;
use super::engine::{Core, lk};

/// «Вернуть боту»: доля оценки переходит к новой сделке (по доле вернувшихся монет), остаток остаётся у старой
/// (монеты, которых нет на балансе, — потеряны). День учёта и пометка «цены нет» сохраняются. Ошибок нет.
pub async fn transfer(core: &Core, from: &mut Deal, to: &mut Deal, share: Decimal) {
    let (today, mut book) = today_book(core);
    let Some((amount, day)) = book.provisional.remove(&from.id) else {
        return;
    };
    let moved = amount * share;
    book.provisional.insert(to.id.clone(), (moved, day));
    if book.unpriced.contains(&from.id) {
        book.unpriced.insert(to.id.clone());
    }
    if amount != moved {
        book.provisional
            .insert(from.id.clone(), (amount - moved, day));
    } else {
        book.unpriced.remove(&from.id);
    }
    from.provisional_usd = Some(amount - moved);
    to.provisional_usd = Some(moved);
    commit(core, today, book).await;
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
            let (Ok(date), Ok(book)) = (
                day.parse::<NaiveDate>(),
                serde_json::from_str::<Book>(&body),
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
            {
                let mut r = lk(&core.inner.risk);
                r.day.date = date;
                r.day.final_usd = book.final_usd;
                r.day.limit_reset = book.limit_reset;
            }
            let (today, mut cur) = today_book(core);
            cur.provisional = book.provisional;
            cur.unpriced = book.unpriced;
            commit(core, today, cur).await;
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
