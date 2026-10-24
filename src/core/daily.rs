//! Файл: core/daily.rs
//! Что делает: итог дня для дневного лимита убытка (решение 53). Закрытая сделка — фактический итог.
//! Остановленная сделка (Halted) и «Беру на себя» — сразу предварительный убыток по худшему варианту:
//! выручка − вложено − затраты (монеты считаются потерянными); он входит в итог дня и может остановить
//! систему (risk.max_daily_loss_usd). У сделки запоминается учтённая сумма и день. Разбор («Продать сейчас»,
//! «Вернуть боту» и продажа, любое закрытие такой сделки): предварительное снимается, добавляется факт.
//! Разбор в другой день — коррекция (факт − предварительное) идёт в итог дня разбора, а день учёта не
//! меняется. Итог дня — таблица daily_result (storage/db.rs), строка на дату; после перезапуска
//! восстанавливается; неразобранные предварительные убытки переносятся в новый день (в его итог не входят).
//! Кто вызывает: ledger.rs (finish), manual_return.rs (перенос доли), engine.rs (load).
//! От чего зависит: risk.rs (DayView, check_daily), deal.rs, storage/db.rs.
//! Настройки: risk.max_daily_loss_usd (через risk.rs).

use std::collections::BTreeMap;

use chrono::{NaiveDate, Utc};
use rusqlite::params;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::{Core, lk};

/// Строка таблицы daily_result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Book {
    final_usd: Decimal,
    /// Неразобранные предварительные убытки: сделка → (сумма, день учёта).
    provisional: BTreeMap<String, (Decimal, NaiveDate)>,
}

/// Пересчитывает DayView из книги дня и сохраняет. Возвращает: Some(текст) — сработал стоп.
async fn commit(core: &Core, date: NaiveDate, book: Book) -> Option<String> {
    let prov: Decimal = book
        .provisional
        .values()
        .filter(|(_, d)| *d == date)
        .map(|(a, _)| *a)
        .sum();
    {
        let mut r = lk(&core.inner.risk);
        r.day.date = date;
        r.day.final_usd = book.final_usd;
        r.day.provisional_usd = prov;
        r.day.result_usd = book.final_usd + prov;
        r.day.provisional = book.provisional.clone();
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
    if let Err(e) = res {
        let m = format!("Итог дня не сохранён: {e}");
        core.inner
            .deps
            .log(Level::Error, "core.daily", None, m, None)
            .await;
    }
    super::risk::check_daily(core)
}

/// Книга на сегодня: смена даты — фактический итог с нуля, неразобранные предварительные — переносятся.
fn today_book(core: &Core) -> (NaiveDate, Book) {
    let today = Utc::now().date_naive();
    let r = lk(&core.inner.risk);
    let fin = if r.day.date == today {
        r.day.final_usd
    } else {
        Decimal::ZERO
    };
    (
        today,
        Book {
            final_usd: fin,
            provisional: r.day.provisional.clone(),
        },
    )
}

/// Предварительный убыток остановленной сделки или «Беру на себя» (по худшему варианту).
/// Принимает: сделку (у неё запоминается учтённая сумма). Возвращает: Some(текст) — сработал стоп.
pub async fn provisional(core: &Core, deal: &mut Deal) -> Option<String> {
    let amount = deal.proceeds - deal.invested - deal.spent;
    let (today, mut book) = today_book(core);
    // Повторный учёт той же сделки — заменяет прежний (снимаем старый, ставим новый на сегодня).
    if let Some((old, day)) = book.provisional.remove(&deal.id)
        && day != today
    {
        book.final_usd -= old;
    }
    book.provisional.insert(deal.id.clone(), (amount, today));
    deal.provisional_usd = Some(amount);
    deal.note(format!(
        "Предварительный убыток в итог дня {today}: {amount} USDT (решение 53)"
    ));
    commit(core, today, book).await
}

/// Факт закрытой сделки: снимает её предварительный убыток (если был) и добавляет факт; разбор в другой
/// день — коррекция в итог дня разбора. Принимает: сделку, факт. Возвращает: Some(текст) — сработал стоп.
pub async fn settle(core: &Core, deal: &mut Deal, fact: Decimal) -> Option<String> {
    let (today, mut book) = today_book(core);
    match book.provisional.remove(&deal.id) {
        Some((_, day)) if day == today => book.final_usd += fact,
        Some((old, _)) => book.final_usd += fact - old,
        None => book.final_usd += fact,
    }
    deal.provisional_usd = None;
    commit(core, today, book).await
}

/// «Вернуть боту»: доля предварительного убытка переходит к новой сделке (по доле вернувшихся монет),
/// остаток остаётся у старой (монеты, которых нет на балансе, — потеряны). День учёта сохраняется.
pub async fn transfer(core: &Core, from: &mut Deal, to: &mut Deal, share: Decimal) {
    let (today, mut book) = today_book(core);
    let Some((amount, day)) = book.provisional.remove(&from.id) else {
        return;
    };
    let moved = amount * share;
    book.provisional.insert(to.id.clone(), (moved, day));
    if amount != moved {
        book.provisional
            .insert(from.id.clone(), (amount - moved, day));
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
            lk(&core.inner.risk).day.date = date;
            lk(&core.inner.risk).day.final_usd = book.final_usd;
            let (today, mut cur) = today_book(core);
            cur.provisional = book.provisional;
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
