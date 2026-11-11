//! Файл: report/day_rows.rs
//! Что делает: решение 257 (Тигран 11.11 п. 3: «итог дня отчёта = итог дня бота») — строки листа «Учёт дня»: каждое
//! начисление сделки в итог дня бота (Deal.day_book, core/deal_book.rs — те же суммы, что core/daily.rs кладёт в книгу
//! дня: факт закрытия, поправки затрат возврата USDT и газа после закрытия — в день, когда случились; оценка
//! остановленной и «Беру на себя»; снятие оценки фактом; коррекция разбора в другой день). Сумма по дню = итог дня бота.
//! Сделки до решения 257 (начислений нет или не все): остаток (факт закрытой / оценка остановленной − сумма начислений)
//! — одной строкой «восстановлено» в день конца сделки (оценка — в день её учёта у бота, если он есть в итоге дня бота);
//! поправки после закрытия у таких сделок по дням не разнесены — это видно в колонке «Разница» листа «По дням».
//! В отбор — все сделки (и открытые: «Вернуть боту» несёт долю оценки), начисления — по времени в окне выгрузки.
//! Кто вызывает: report/mod.rs (results), report/results_xlsx.rs.
//! От чего зависит: core/deal.rs, core/deal_book.rs, report/rows.rs (f, coin_labels), report/rows_text.rs,
//! report/rows_time.rs, notify/human.rs.
//! Настройки: нет.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;

use crate::core::deal::{Deal, Step};
use crate::core::deal_book::DayPart;
use crate::notify::human::venue;

use super::rows::{coin_labels, f};
use super::rows_text::owner_pair;
use super::rows_time::{Window, ended_at};

/// Строка листа «Учёт дня».
#[derive(Debug, Clone, PartialEq)]
pub struct DayRow {
    pub at: DateTime<Utc>,
    pub date: NaiveDate,
    pub deal: String,
    pub coin: String,
    pub direction: String,
    pub owner: String,
    /// true — факт; false — оценка.
    pub fact: bool,
    pub amount: f64,
    pub what: String,
    /// true — из учёта сделки; false — восстановлено (сделка до решения 257).
    pub booked: bool,
}

/// Строки «Учёт дня». Принимает: все сделки, окно, id площадок DEX, дни учёта неразобранных оценок у бота
/// (DayView.provisional: сделка → день). Возвращает: строки по времени. Ошибок нет.
pub fn day_rows(
    deals: &[Deal],
    w: &Window,
    dex: &BTreeSet<String>,
    prov_days: &BTreeMap<String, NaiveDate>,
) -> Vec<DayRow> {
    let labels = coin_labels(deals);
    let mut out: Vec<DayRow> = Vec::new();
    for d in deals {
        let base = DayRow {
            at: d.started_at,
            date: d.started_at.date_naive(),
            deal: d.id.clone(),
            coin: labels.get(&d.id).cloned().unwrap_or_default(),
            direction: format!(
                "{} → {}",
                venue(&d.direction.buy_on.0),
                venue(&d.direction.sell_on.0)
            ),
            owner: owner_pair(d, dex),
            fact: true,
            amount: 0.0,
            what: String::new(),
            booked: true,
        };
        for e in &d.day_book {
            out.push(DayRow {
                at: e.at,
                date: e.date,
                fact: e.part == DayPart::Fact,
                amount: f(e.amount),
                what: e.what.clone(),
                ..base.clone()
            });
        }
        if let Some(r) = rest(d, prov_days, base) {
            out.push(r);
        }
    }
    out.retain(|r| w.has(r.at));
    out.sort_by_key(|r| r.at);
    out
}

/// Остаток сделки до решения 257 (см. шапку). Ошибок нет: остатка нет — None.
fn rest(d: &Deal, prov_days: &BTreeMap<String, NaiveDate>, base: DayRow) -> Option<DayRow> {
    let booked: Decimal = d.day_book.iter().map(|e| e.amount).sum();
    let (target, fact) = match d.step {
        Step::Closed => (d.result_usd?, true),
        _ => (d.provisional_usd?, false),
    };
    let left = target - booked;
    if left.is_zero() {
        return None;
    }
    let at = ended_at(d).unwrap_or(d.started_at);
    let date = if fact {
        at.date_naive()
    } else {
        prov_days.get(&d.id).copied().unwrap_or(at.date_naive())
    };
    Some(DayRow {
        at,
        date,
        fact,
        amount: f(left),
        what: if fact {
            "восстановлено: итог сделки без начислений по дням (сделка до решения 257) — в день закрытия".into()
        } else {
            "восстановлено: оценка без начислений по дням (сделка до решения 257)".into()
        },
        booked: false,
        ..base
    })
}
