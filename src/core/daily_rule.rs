//! Файл: core/daily_rule.rs
//! Что делает: t91 (Тигран 11.11.2026: «биржа ↔ биржа — бирже покупки, как в отчёте») — приведение загруженной записи
//! итога дня, сделанной по прежнему правилу (решение 173: сделка биржа ↔ биржа — каждой из двух бирж весь итог; в записи
//! нет поля venue_rule), к новому правилу (daily_venue::owner — только бирже покупки). Прошлые дни не пересчитываются
//! (их строки daily_result не переписываются); меняется только книга, которая станет сегодняшней:
//! - площадки неразобранных оценок сделок биржа ↔ биржа — по новому правилу (Book.deal_venues; оценка теперь у биржи
//!   покупки, её разбор — тоже);
//! - запись сегодняшняя — из факта дня биржи ПРОДАЖИ снимается то, что ей начислили сделки биржа ↔ биржа (у биржи
//!   покупки оно остаётся — двойной счёт уходит, ничего не теряется; общий итог дня не меняется); отметка сброса лимита
//!   биржи продажи (итог в момент сброса) уменьшается на их часть, начисленную до сброса.
//!
//! Сколько начислено: по начислениям сделки в итог дня (Deal.day_book, решение 257) — точно; у сделок без начислений
//! (до решения 257) — как разносит daily_migrate.rs: факт = result_usd закрытой сделки с днём последнего изменения
//! (updated_at) = день записи, оценка = её сумма в книге, время — updated_at; поправка оценки прошлого дня, разобранной
//! сегодня, так не видна (у такой сделки снимается весь result_usd) — число таких сделок «по оценке» пишется в журнал.
//! Кто вызывает: daily_load.rs (load).
//! От чего зависит: daily.rs (Book), daily_venue.rs (is_cex_cex, of_direction), deal_book.rs (DayPart), persist.rs,
//! ledger.rs (KIND), deal.rs.
//! Настройки: нет.

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;

use crate::storage::journal::Level;

use super::daily::Book;
use super::deal::{Deal, Step};
use super::deal_book::DayPart;
use super::engine::Core;

/// Начисление сделки в итог дня записи: время, сумма, факт (иначе — оценка).
struct Part {
    at: DateTime<Utc>,
    amount: Decimal,
    fact: bool,
}

/// Что делает: приводит книгу по прежнему правилу к новому (см. шапку). Принимает: ядро, день записи, сегодня, книгу.
/// Ошибки: сделки не прочитаны — в журнал, книга остаётся как была (общий итог цел).
pub(super) async fn fix(core: &Core, date: NaiveDate, today: NaiveDate, book: &mut Book) {
    let deps = &core.inner.deps;
    let deals = match super::persist::load::<Deal>(&deps.db, super::ledger::KIND).await {
        Ok(d) => d,
        Err(e) => {
            let m = format!(
                "Итог дня {date}: разбивка по площадкам не приведена к правилу «биржа ↔ биржа — бирже покупки»: \
                 сделки не прочитаны ({e})"
            );
            deps.log(Level::Error, "core.daily", None, m, None).await;
            return;
        }
    };
    let (mut n, mut moved, mut guessed) = (0usize, Decimal::ZERO, 0usize);
    for d in deals {
        if !super::daily_venue::is_cex_cex(core, &d.direction) {
            continue;
        }
        if let Some(v) = book.deal_venues.get_mut(&d.id) {
            *v = super::daily_venue::of_direction(core, &d.direction);
        }
        if date != today {
            continue;
        }
        let (parts, exact) = parts(&d, date, book);
        if parts.is_empty() {
            continue;
        }
        let sell = &d.direction.sell_on.0;
        let fact: Decimal = parts.iter().filter(|p| p.fact).map(|p| p.amount).sum();
        if !fact.is_zero() {
            *book.venue_final.entry(sell.clone()).or_default() -= fact;
        }
        if let Some(r) = book.venue_resets.get_mut(sell) {
            let before: Decimal = parts
                .iter()
                .filter(|p| p.at <= r.at)
                .map(|p| p.amount)
                .sum();
            r.base_usd -= before;
        }
        n += 1;
        moved += fact;
        if !exact {
            guessed += 1;
        }
    }
    let m = format!(
        "Итог дня {date}: разбивка по площадкам приведена к правилу «биржа ↔ биржа — бирже покупки» (t91): сделок биржа ↔ \
         биржа дня {n} (из них по оценке, без начислений решения 257, — {guessed}), с бирж продажи снят факт {} USDT; \
         общий итог {} USDT не меняется; прошлые дни не пересчитываются",
        moved.round_dp(4),
        book.final_usd.round_dp(4)
    );
    deps.log(Level::Info, "core.daily", None, m, None).await;
}

/// Что делает: начисления сделки в итог дня `date`. Принимает: сделку, день, книгу (её оценки).
/// Возвращает: (начисления, точно ли — по Deal.day_book). Ошибок нет.
fn parts(d: &Deal, date: NaiveDate, book: &Book) -> (Vec<Part>, bool) {
    if !d.day_book.is_empty() {
        let p = d
            .day_book
            .iter()
            .filter(|e| e.date == date)
            .map(|e| Part {
                at: e.at,
                amount: e.amount,
                fact: e.part == DayPart::Fact,
            })
            .collect();
        return (p, true);
    }
    let mut p = Vec::new();
    if let Some(r) = d.result_usd
        && d.step == Step::Closed
        && d.updated_at.date_naive() == date
    {
        p.push(Part {
            at: d.updated_at,
            amount: r,
            fact: true,
        });
    }
    if let Some((a, day)) = book.provisional.get(&d.id)
        && *day == date
    {
        p.push(Part {
            at: d.updated_at,
            amount: *a,
            fact: false,
        });
    }
    (p, false)
}
