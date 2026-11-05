//! Файл: core/daily_migrate.rs
//! Что делает: разнесение старой записи итога дня по площадкам (решение 173) при загрузке. Запись до решения 173 —
//! без разбивки (нет поля venue_final): факт дня каждой площадки = сумма итогов сделок, закрытых в этот день
//! (result_usd, день последнего изменения сделки), с её участием (CEX ↔ DEX — бирже, CEX ↔ CEX — обеим, DEX ↔ DEX —
//! DEX; daily_venue.rs); у неразобранных оценок — площадки их сделок. Общий итог дня не меняется. Разница между общим
//! фактом и суммой сделок дня (поправки разборов сделок прошлых дней) в разбивку не попадает — пишется в журнал.
//! Кто вызывает: daily_load.rs (load).
//! От чего зависит: daily.rs (Book), daily_venue.rs (of_direction, add_final), persist.rs, ledger.rs (KIND), deal.rs.
//! Настройки: нет.

use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::storage::journal::Level;

use super::daily::Book;
use super::deal::{Deal, Step};
use super::engine::Core;

/// Что делает: разносит книгу дня по площадкам сделок из учёта. Принимает: ядро, день книги, книгу.
/// Ошибки: сделки не прочитаны — в журнал, книга остаётся без разбивки (общий итог цел).
pub(super) async fn split(core: &Core, date: NaiveDate, book: &mut Book) {
    let deps = &core.inner.deps;
    let deals = match super::persist::load::<Deal>(&deps.db, super::ledger::KIND).await {
        Ok(d) => d,
        Err(e) => {
            let m = format!("Итог дня {date} не разнесён по площадкам: сделки не прочитаны ({e})");
            deps.log(Level::Error, "core.daily", None, m, None).await;
            return;
        }
    };
    let (mut n, mut sum) = (0usize, Decimal::ZERO);
    for d in deals {
        let venues = super::daily_venue::of_direction(core, &d.direction);
        if book.provisional.contains_key(&d.id) {
            book.deal_venues.insert(d.id.clone(), venues.clone());
        }
        if let Some(r) = d.result_usd
            && d.step == Step::Closed
            && d.updated_at.date_naive() == date
        {
            super::daily_venue::add_final(book, &venues, r);
            n += 1;
            sum += r;
        }
    }
    let rest = book.final_usd - sum;
    let rest = if rest.is_zero() {
        String::new()
    } else {
        format!(
            "; разница {} USDT (поправки разборов прошлых дней) — только в общем итоге",
            rest.round_dp(4)
        )
    };
    let m = format!(
        "Итог дня {date} разнесён по площадкам (решение 173): закрытых сделок дня {n}, их факт {} USDT; общий итог \
         {} USDT не меняется{rest}",
        sum.round_dp(4),
        book.final_usd.round_dp(4)
    );
    deps.log(Level::Info, "core.daily", None, m, None).await;
}
