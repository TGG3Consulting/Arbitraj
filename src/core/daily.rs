//! Файл: core/daily.rs
//! Что делает: итог дня (решения 53, 152, 173). Итог дня = факт закрытых сегодня сделок (result_usd) + оценка позиций, которые
//! бот сам не довёл: остановленная сделка (Halted) и «Беру на себя». Оценка позиции — стоимость её монет по рынку
//! площадки, где они лежат, − вложено − затраты (daily_value.rs); цены нет — худший случай (вся сумма) с пометкой
//! (DayView.unpriced). У сделки запоминается учтённая оценка и день. Разбор («Продать сейчас», «Продать и перегнать»,
//! «Продано вручную», «Вернуть боту» и продажа — любое закрытие этой сделки): её оценка снимается ровно на эту сделку,
//! добавляется факт. Разбор в другой день — коррекция (факт − оценка) идёт в итог дня разбора, день учёта не меняется.
//! Итог дня — таблица daily_result (storage/db.rs), строка на дату; после перезапуска восстанавливается; неразобранные
//! оценки переносятся в новый день (в его итог не входят). Там же — отметка сброса лимита дня (решение 153,
//! daily_limit.rs): действует только в свой день.
//! Решение 173: у итога дня — разбивка по площадкам (daily_venue.rs): факт сделки и её оценка идут каждой её площадке
//! (CEX ↔ DEX — бирже, CEX ↔ CEX — обеим биржам, DEX ↔ DEX — DEX); общий итог — по-прежнему сумма сделок.
//! Пересчёт, запись в базу и проверка лимитов — daily_commit.rs.
//! Кто вызывает: ledger.rs (finish), ledger_load.rs, manual_return.rs (перенос доли), engine.rs (load), daily_limit.rs,
//! daily_venue.rs.
//! От чего зависит: daily_value.rs (оценка), daily_commit.rs, daily_venue.rs, day_view.rs, deal.rs.
//! Настройки: risk.max_daily_loss_usd, risk.max_daily_loss_total_usd (через daily_venue.rs, daily_limit.rs).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::daily_commit::Trip;
use super::daily_value::Estimate;
use super::day_view::LimitReset;
use super::deal::Deal;
use super::engine::{Core, lk};

pub(super) use super::daily_commit::commit;
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
    /// Сброс общего лимита дня (решение 153) — только этого дня.
    #[serde(default)]
    pub(super) limit_reset: Option<LimitReset>,
    /// Решение 173: факт дня по площадкам (id площадки → сумма). Нет поля в старой записи — разнесение при загрузке
    /// (daily_migrate.rs).
    #[serde(default)]
    pub(super) venue_final: BTreeMap<String, Decimal>,
    /// Решение 173: площадки неразобранных оценок (сделка → id площадок).
    #[serde(default)]
    pub(super) deal_venues: BTreeMap<String, Vec<String>>,
    /// Решение 173: сброс лимита дня площадки (id → отметка) — только этого дня.
    #[serde(default)]
    pub(super) venue_resets: BTreeMap<String, LimitReset>,
}

/// Сумма оценок, учтённых в день `date`. Ошибок нет.
pub(super) fn prov_on(book: &Book, date: NaiveDate) -> Decimal {
    book.provisional
        .values()
        .filter(|(_, d)| *d == date)
        .map(|(a, _)| *a)
        .sum()
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
            venue_final: if same {
                super::daily_venue::finals(&r.day)
            } else {
                BTreeMap::new()
            },
            deal_venues: r.day.deal_venues.clone(),
            venue_resets: if same {
                super::daily_venue::resets(&r.day)
            } else {
                BTreeMap::new()
            },
        },
    )
}

/// Оценка позиции остановленной сделки или «Беру на себя» (решение 152): стоимость монет по рынку площадки, где они
/// лежат; цены нет — худший случай с пометкой. Принимает: сделку. Возвращает: что сработало (лимиты, Trip).
pub async fn provisional(core: &Core, deal: &mut Deal) -> Trip {
    let value = super::daily_value::value_now(core, deal).await;
    let est = super::daily_value::estimate(deal, value);
    record(core, deal, est).await
}

/// Записывает оценку позиции в итог дня (повторный учёт той же сделки заменяет прежний). Принимает: сделку (у неё
/// запоминается сумма), оценку (daily_value::estimate). Возвращает: что сработало (лимиты, Trip).
pub async fn record(core: &Core, deal: &mut Deal, est: Estimate) -> Trip {
    let (today, mut book) = today_book(core);
    let venues = super::daily_venue::of_deal(core, &book, deal);
    if let Some((old, day)) = book.provisional.remove(&deal.id)
        && day != today
    {
        book.final_usd -= old;
        super::daily_venue::add_final(&mut book, &venues, -old);
    }
    book.provisional
        .insert(deal.id.clone(), (est.amount, today));
    book.deal_venues.insert(deal.id.clone(), venues);
    if est.priced {
        book.unpriced.remove(&deal.id);
    } else {
        book.unpriced.insert(deal.id.clone());
    }
    deal.provisional_usd = Some(est.amount);
    deal.note(format!(
        "В итог дня {today}: {} USDT — {}",
        crate::notify::human::money(est.amount),
        est.note
    ));
    commit(core, today, book).await
}

/// Факт закрытой сделки: снимает её оценку (если была) и добавляет факт; разбор в другой день — коррекция в итог дня
/// разбора. Решение 173: та же сумма — каждой площадке сделки. Принимает: сделку, факт.
/// Возвращает: что сработало (лимиты, Trip).
pub async fn settle(core: &Core, deal: &mut Deal, fact: Decimal) -> Trip {
    let (today, mut book) = today_book(core);
    let venues = super::daily_venue::of_deal(core, &book, deal);
    let delta = match book.provisional.remove(&deal.id) {
        Some((_, day)) if day == today => fact,
        Some((old, _)) => fact - old,
        None => fact,
    };
    book.final_usd += delta;
    super::daily_venue::add_final(&mut book, &venues, delta);
    book.deal_venues.remove(&deal.id);
    book.unpriced.remove(&deal.id);
    deal.provisional_usd = None;
    commit(core, today, book).await
}
