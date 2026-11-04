//! Файл: core/daily.rs
//! Что делает: итог дня (решения 53, 152). Итог дня = факт закрытых сегодня сделок (result_usd) + оценка позиций, которые
//! бот сам не довёл: остановленная сделка (Halted) и «Беру на себя». Оценка позиции — стоимость её монет по рынку
//! площадки, где они лежат, − вложено − затраты (daily_value.rs); цены нет — худший случай (вся сумма) с пометкой
//! (DayView.unpriced). У сделки запоминается учтённая оценка и день. Разбор («Продать сейчас», «Продать и перегнать»,
//! «Продано вручную», «Вернуть боту» и продажа — любое закрытие этой сделки): её оценка снимается ровно на эту сделку,
//! добавляется факт. Разбор в другой день — коррекция (факт − оценка) идёт в итог дня разбора, день учёта не меняется.
//! Итог дня — таблица daily_result (storage/db.rs), строка на дату; после перезапуска восстанавливается; неразобранные
//! оценки переносятся в новый день (в его итог не входят). Там же — отметка сброса лимита дня (решение 153,
//! daily_limit.rs): действует только в свой день.
//! Кто вызывает: ledger.rs (finish), ledger_load.rs, manual_return.rs (перенос доли), engine.rs (load), daily_limit.rs.
//! От чего зависит: daily_value.rs (оценка), daily_limit.rs (лимит), day_view.rs, deal.rs, storage/db.rs.
//! Настройки: risk.max_daily_loss_usd (через daily_limit.rs).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{NaiveDate, Utc};
use rusqlite::params;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::storage::journal::Level;

use super::daily_value::Estimate;
use super::day_view::LimitReset;
use super::deal::Deal;
use super::engine::{Core, lk};

pub use super::daily_load::{load, transfer};

/// Строка таблицы daily_result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Book {
    pub(super) final_usd: Decimal,
    /// Неразобранные оценки позиций: сделка → (сумма, день учёта).
    pub(super) provisional: BTreeMap<String, (Decimal, NaiveDate)>,
    /// Сделки, чья оценка — худший случай (цены не было; решение 152).
    #[serde(default)]
    pub(super) unpriced: BTreeSet<String>,
    /// Сброс лимита дня (решение 153) — только этого дня.
    #[serde(default)]
    pub(super) limit_reset: Option<LimitReset>,
}

/// Сумма оценок, учтённых в день `date`. Ошибок нет.
pub(super) fn prov_on(book: &Book, date: NaiveDate) -> Decimal {
    book.provisional
        .values()
        .filter(|(_, d)| *d == date)
        .map(|(a, _)| *a)
        .sum()
}

/// Пересчитывает DayView из книги дня, сохраняет и проверяет лимит. Возвращает: Some(текст) — сработал стоп.
pub(super) async fn commit(core: &Core, date: NaiveDate, book: Book) -> Option<String> {
    let prov = prov_on(&book, date);
    {
        let mut r = lk(&core.inner.risk);
        r.day.date = date;
        r.day.final_usd = book.final_usd;
        r.day.provisional_usd = prov;
        r.day.result_usd = book.final_usd + prov;
        r.day.provisional = book.provisional.clone();
        r.day.unpriced = book.unpriced.clone();
        r.day.limit_reset = book.limit_reset.clone();
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
    let (stop, cleared) = super::daily_limit::check(core);
    // Решение 152: факт пришёл, лимит больше не превышен — остановку снимает человек, но он должен это видеть.
    if let Some(t) = cleared {
        core.inner
            .deps
            .log(Level::Info, "core.daily_loss", None, t, None)
            .await;
    }
    stop
}

/// Книга на сегодня: смена даты — факт с нуля, сброса лимита нет; неразобранные оценки — переносятся.
pub(super) fn today_book(core: &Core) -> (NaiveDate, Book) {
    let today = Utc::now().date_naive();
    let r = lk(&core.inner.risk);
    let same = r.day.date == today;
    (
        today,
        Book {
            final_usd: if same { r.day.final_usd } else { Decimal::ZERO },
            provisional: r.day.provisional.clone(),
            unpriced: r.day.unpriced.clone(),
            limit_reset: if same {
                r.day.limit_reset.clone()
            } else {
                None
            },
        },
    )
}

/// Оценка позиции остановленной сделки или «Беру на себя» (решение 152): стоимость монет по рынку площадки, где они
/// лежат; цены нет — худший случай с пометкой. Принимает: сделку. Возвращает: Some(текст) — сработал стоп.
pub async fn provisional(core: &Core, deal: &mut Deal) -> Option<String> {
    let value = super::daily_value::value_now(core, deal).await;
    let est = super::daily_value::estimate(deal, value);
    record(core, deal, est).await
}

/// Записывает оценку позиции в итог дня (повторный учёт той же сделки заменяет прежний). Принимает: сделку (у неё
/// запоминается сумма), оценку (daily_value::estimate). Возвращает: Some(текст) — сработал стоп.
pub async fn record(core: &Core, deal: &mut Deal, est: Estimate) -> Option<String> {
    let (today, mut book) = today_book(core);
    if let Some((old, day)) = book.provisional.remove(&deal.id)
        && day != today
    {
        book.final_usd -= old;
    }
    book.provisional
        .insert(deal.id.clone(), (est.amount, today));
    if est.priced {
        book.unpriced.remove(&deal.id);
    } else {
        book.unpriced.insert(deal.id.clone());
    }
    deal.provisional_usd = Some(est.amount);
    deal.note(format!(
        "В итог дня {today}: {} USDT — {} (решение 152)",
        est.amount.round_dp(6),
        est.note
    ));
    commit(core, today, book).await
}

/// Факт закрытой сделки: снимает её оценку (если была) и добавляет факт; разбор в другой день — коррекция в итог дня
/// разбора. Принимает: сделку, факт. Возвращает: Some(текст) — сработал стоп.
pub async fn settle(core: &Core, deal: &mut Deal, fact: Decimal) -> Option<String> {
    let (today, mut book) = today_book(core);
    match book.provisional.remove(&deal.id) {
        Some((_, day)) if day == today => book.final_usd += fact,
        Some((old, _)) => book.final_usd += fact - old,
        None => book.final_usd += fact,
    }
    book.unpriced.remove(&deal.id);
    deal.provisional_usd = None;
    commit(core, today, book).await
}
