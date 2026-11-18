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
//! Решение 258 (было — по площадкам, решение 173): у итога дня — разбивка по парам «площадка × тип направления»
//! (daily_venue.rs, day_pair.rs): факт сделки и её оценка идут её паре (биржа → DEX, DEX → биржа — бирже этого
//! направления; биржа → биржа — бирже покупки; DEX ↔ DEX — общей паре «DEX ↔ DEX»); общий итог — сумма сделок.
//! Пересчёт, запись в базу и проверка лимитов — daily_commit.rs.
//! Решение 414: в той же строке — просадка пар от пика (Book.draw, Book.draw_want; core/drawdown*.rs), не дневная.
//! Решение 257: каждое изменение итога дня из-за сделки — ещё и начисление в её учёт (Deal.day_book, deal_book.rs):
//! отчёт «Итоги сделок» складывает их по дням и получает итог дня бота.
//! Кто вызывает: ledger.rs (finish), ledger_load.rs, manual_return.rs (перенос доли), engine.rs (load), daily_limit.rs,
//! daily_venue.rs, daily_migrate.rs.
//! От чего зависит: daily_value.rs (оценка), daily_commit.rs, daily_venue.rs, day_view.rs, deal.rs, deal_book.rs
//! (начисления в учёт сделки, решение 257).
//! Настройки: risk.max_daily_loss_usd, risk.max_daily_loss_total_usd (через daily_venue_limit.rs, daily_limit.rs).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::daily_commit::Trip;
use super::daily_value::Estimate;
use super::day_view::LimitReset;
use super::deal::Deal;
use super::deal_book::{DayPart, day as day_entry};
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
    /// Решение 258: факт дня по парам «площадка × тип направления» (id пары → сумма; day_pair.rs). Пишется всегда (и
    /// пустым): нет поля — запись до решения 258 (по площадкам: venue_final / deal_venues / venue_resets или без
    /// разбивки) — разнесение по парам при загрузке (daily_migrate.rs).
    #[serde(default)]
    pub(super) pair_final: BTreeMap<String, Decimal>,
    /// Решение 258: пара неразобранной оценки (сделка → id пары).
    #[serde(default)]
    pub(super) deal_pairs: BTreeMap<String, String>,
    /// Решение 258: сброс лимита дня пары (id пары → отметка) — только этого дня.
    #[serde(default)]
    pub(super) pair_resets: BTreeMap<String, LimitReset>,
    /// Решение 414: просадка пар от пика (слепок, баланс, пик — core/drawdown.rs). Не дневное: переносится в новый день
    /// (новый слепок снимается заново — drawdown_snap.rs; пока не снят, действует прежний пик).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) draw: BTreeMap<String, super::drawdown::DrawState>,
    /// Решение 414: запрошенные и ещё не снятые слепки пар (переживают перезапуск — задача снятия поднимается снова).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) draw_want: BTreeMap<String, super::drawdown::Want>,
    /// Решение 414 (ревью t199, мелкое 6): паузы направлений по просадке (id пары → причина) и просадка при прошлом
    /// пересчёте — переживают перезапуск (без ложной паузы и без повторной после «Снять паузу»). Пишет commit из
    /// Risk.draw_paused / Risk.draw_seen; при запуске восстанавливает drawdown_limit::restore.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) draw_paused: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) draw_seen: BTreeMap<String, Decimal>,
    /// Ревью t199, С1: сделка, ради которой пересчитывается итог дня, и её плановые затраты возврата (в базу не пишется;
    /// ставят settle и record — drawdown_pending.rs cur).
    #[serde(skip)]
    pub(super) cur: Option<super::drawdown_pending::Cur>,
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
            pair_final: if same {
                super::daily_venue::finals(&r.day)
            } else {
                BTreeMap::new()
            },
            deal_pairs: r.day.deal_pairs.clone(),
            pair_resets: if same {
                super::daily_venue::resets(&r.day)
            } else {
                BTreeMap::new()
            },
            // Решение 414: просадка не дневная — переносится всегда.
            draw: r.day.draw.clone(),
            draw_want: r.day.draw_want.clone(),
            // Паузы по просадке и замеченную просадку пишет commit из Risk; сделку пересчёта (cur) ставят settle и
            // record.
            ..Book::default()
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
    let pair = super::daily_venue::of_deal(core, &book, deal);
    // Ревью t199, С1: плановые затраты возврата этой сделки — из неё самой (в учёте ядра она ещё прежняя).
    book.cur = Some(super::drawdown_pending::cur(core, deal, false));
    // Решение 257: каждое изменение итога дня из-за сделки — начислением в её учёт (deal_book.rs).
    match book.provisional.remove(&deal.id) {
        Some((old, day)) if day != today => {
            book.final_usd -= old;
            super::daily_venue::add_final(&mut book, &pair, -old);
            day_entry(
                deal,
                today,
                DayPart::Fact,
                -old,
                "оценка прошлого дня снята",
            );
        }
        Some((old, _)) => day_entry(deal, today, DayPart::Estimate, -old, "прежняя оценка снята"),
        None => {}
    }
    day_entry(deal, today, DayPart::Estimate, est.amount, "оценка позиции");
    book.provisional
        .insert(deal.id.clone(), (est.amount, today));
    book.deal_pairs.insert(deal.id.clone(), pair);
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
/// разбора. Решение 258: та же сумма — паре сделки (daily_venue::of_deal). Принимает: сделку, факт.
/// Возвращает: что сработало (лимиты, Trip).
pub async fn settle(core: &Core, deal: &mut Deal, fact: Decimal) -> Trip {
    settle_as(core, deal, fact, false).await
}

/// Что делает: settle самого закрытия сделки (ledger::finish): решение 414 (повторное ревью t199) — плановые затраты
/// возврата «как решит prepare» (drawdown_pending.rs cur с closing) и держатся в Inner.draw_closing до сохранения
/// сделки (снимает ledger::finish). Принимает: сделку, факт. Возвращает: что сработало (лимиты, Trip).
pub async fn settle_close(core: &Core, deal: &mut Deal, fact: Decimal) -> Trip {
    settle_as(core, deal, fact, true).await
}

/// settle с признаком «само закрытие» (см. settle_close). Принимает: сделку, факт, признак. Возвращает: Trip.
async fn settle_as(core: &Core, deal: &mut Deal, fact: Decimal, closing: bool) -> Trip {
    let (today, mut book) = today_book(core);
    let pair = super::daily_venue::of_deal(core, &book, deal);
    // Ревью t199, С1: закрываемая сделка ещё без частей возврата в учёте ядра — её плановые затраты возврата сама;
    // само закрытие — ещё и для пересчётов других сделок до сохранения (Inner.draw_closing).
    let cur = super::drawdown_pending::cur(core, deal, closing);
    if closing {
        super::drawdown_pending::closing(core, &deal.id, Some(cur.clone()));
    }
    book.cur = Some(cur);
    // Решение 257: начисления в учёт сделки — те же суммы, что идут в книгу дня (deal_book.rs).
    let delta = match book.provisional.remove(&deal.id) {
        Some((old, day)) if day == today => {
            day_entry(deal, today, DayPart::Estimate, -old, "оценка снята фактом");
            fact
        }
        Some((old, _)) => fact - old,
        None => fact,
    };
    day_entry(deal, today, DayPart::Fact, delta, "факт");
    book.final_usd += delta;
    super::daily_venue::add_final(&mut book, &pair, delta);
    book.deal_pairs.remove(&deal.id);
    book.unpriced.remove(&deal.id);
    deal.provisional_usd = None;
    commit(core, today, book).await
}
