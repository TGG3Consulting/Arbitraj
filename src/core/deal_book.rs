//! Файл: core/deal_book.rs
//! Что делает: решение 257 (Тигран 11.11.2026, отчёт «Итоги сделок») — две записи учёта сделки для отчёта:
//! - статьи затрат по факту (Deal.cost_items): каждая сумма, которая вошла в затраты сделки (spent), — с видом затраты
//!   (торговая комиссия, газ обмена, approve, чаевые канала, комиссия вывода, газ перевода …) и временем. Сумма статей =
//!   spent у сделок, открытых после решения 257; у старых — статей нет (в отчёте «без разбивки»);
//! - начисления в итог дня бота (Deal.day_book): каждая сумма, которую daily.rs (решения 53, 152, 173, 236) добавил в
//!   итог дня из-за этой сделки, — день UTC, часть (факт или оценка), сумма. Сумма начислений одного дня по всем сделкам =
//!   итог дня бота (факт дня — части «факт», оценка дня — части «оценка»), поэтому отчёт показывает итог дня как бот.
//!
//! Записи только дополняют учёт: итог сделки, итог дня и лимиты считаются как раньше.
//! Кто вызывает: fills.rs, deal_gas.rs, cycle_deal.rs (spend, spend_bnb), daily.rs, daily_load.rs (day),
//! deal_return_cost.rs, deal_gas.rs (settle_closed — поправка закрытой сделки на копии и её начисление), manual_return.rs
//! (share), ledger.rs (end_prefix — строка закрытия в хронологии), report/*.
//! От чего зависит: deal.rs, daily.rs (settle), ledger.rs (save), deal_gas.rs (Wasted), deal_text.rs, core/types
//! (CostKind), engine.rs.
//! Настройки: нет.

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::CostKind;

use super::deal::Deal;
use super::engine::{Core, lk};

/// Статья затрат сделки по факту: вид и сколько USDT вошло в spent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostItem {
    pub kind: CostKind,
    pub usd: Decimal,
    pub at: DateTime<Utc>,
}

/// Часть итога дня бота (DayView): факт (final_usd) или оценка позиций (provisional_usd).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DayPart {
    Fact,
    Estimate,
}

/// Начисление в итог дня бота из-за сделки: день UTC, часть, сумма (со знаком), что это.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DayEntry {
    pub at: DateTime<Utc>,
    pub date: NaiveDate,
    pub part: DayPart,
    pub amount: Decimal,
    pub what: String,
}

/// Что делает: затрата в USDT — в spent и статьёй. Принимает: сделку, вид, сумму USDT. Ошибок нет.
pub fn spend(deal: &mut Deal, kind: CostKind, usd: Decimal) {
    deal.spent += usd;
    if !usd.is_zero() {
        deal.cost_items.push(CostItem {
            kind,
            usd,
            at: Utc::now(),
        });
    }
}

/// Что делает: статьи газа в BNB, пересчитанные одной суммой `usd` (одна цена BNB на все), — в spent одной суммой `usd`
/// и статьями по доле BNB каждой (последней — остаток, чтобы сумма статей была ровно `usd`).
/// Принимает: сделку, статьи (вид, BNB), сумму USDT. Ошибок нет.
pub fn spend_bnb(deal: &mut Deal, items: &[(CostKind, Decimal)], usd: Decimal) {
    let total: Decimal = items.iter().map(|(_, b)| *b).sum();
    if total.is_zero() {
        spend(deal, items.first().map_or(CostKind::Gas, |x| x.0), usd);
        return;
    }
    let mut left = usd;
    for (k, (kind, bnb)) in items.iter().enumerate() {
        let part = if k + 1 == items.len() {
            left
        } else {
            usd * *bnb / total
        };
        left -= part;
        spend(deal, *kind, part);
    }
}

/// Что делает: статьи газа deal_gas.rs (вид и BNB каждой), пересчитанные одной суммой `usd`, — как spend_bnb.
/// Принимает: сделку, статьи, сумму USDT. Ошибок нет.
pub fn spend_wasted(deal: &mut Deal, list: &[super::deal_gas::Wasted], usd: Decimal) {
    let items: Vec<(CostKind, Decimal)> = list.iter().map(|w| (w.kind, w.bnb)).collect();
    spend_bnb(deal, &items, usd);
}

/// Что делает: «Вернуть боту» (manual_return.rs) — новой сделке переходит доля затрат старой: статьи × доля.
/// Принимает: статьи старой сделки, долю. Возвращает: статьи новой. Ошибок нет.
pub fn share(items: &[CostItem], share: Decimal) -> Vec<CostItem> {
    items
        .iter()
        .map(|c| CostItem {
            usd: c.usd * share,
            ..c.clone()
        })
        .collect()
}

/// Что делает: начисление в итог дня (вызывает только daily.rs / daily_load.rs — рядом с изменением книги дня).
/// Принимает: сделку, день, часть, сумму, что это. Нулевая сумма не пишется. Ошибок нет.
pub fn day(deal: &mut Deal, date: NaiveDate, part: DayPart, amount: Decimal, what: &str) {
    if amount.is_zero() {
        return;
    }
    deal.day_book.push(DayEntry {
        at: Utc::now(),
        date,
        part,
        amount,
        what: what.to_string(),
    });
}

/// Что делает: поправка итога закрытой сделки (затраты возврата USDT, газ после закрытия) — daily::settle на копии
/// сделки и начисление — в сделку учёта (keep_day). Принимает: ядро, копию, поправку. Возвращает: что сработало (Trip).
pub async fn settle_closed(core: &Core, x: &mut Deal, delta: Decimal) -> super::daily_commit::Trip {
    let before = x.day_book.len();
    let trip = super::daily::settle(core, x, delta).await;
    keep_day(core, x, before).await;
    trip
}

/// Что делает: начисления, которые daily::settle записал в КОПИЮ закрытой сделки (поправка затрат возврата USDT или газа
/// после закрытия: deal_return_cost::after, deal_gas::charge_closed), — в сделку учёта в памяти и в базу.
/// Принимает: ядро, копию после settle, сколько начислений было у копии до settle. Ошибок нет: сделки нет — ничего.
pub async fn keep_day(core: &Core, x: &Deal, before: usize) {
    let new: Vec<DayEntry> = x.day_book.iter().skip(before).cloned().collect();
    if new.is_empty() {
        return;
    }
    let d = {
        let mut deals = lk(&core.inner.deals);
        let Some(d) = deals.get_mut(&x.id) else {
            return;
        };
        d.day_book.extend(new);
        d.clone()
    };
    super::ledger::save(core, &d).await;
}

/// Начало строки хронологии закрытия сделки шагом `step` («Сделка закрыта: », «Сделка остановлена: » …; пишет
/// ledger::finish) — по ней отчёт «Итоги сделок» берёт время конца сделки (report/rows_time.rs). Ошибок нет.
pub fn end_prefix(s: super::deal::Step) -> String {
    format!("Сделка {}: ", super::deal_text::step_name(s))
}
