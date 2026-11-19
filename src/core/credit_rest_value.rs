//! Файл: core/credit_rest_value.rs
//! Что делает: решение 413 × 414 (ревью слияния, Н-1) — оценка позиции в итоге дня (решения 53, 152) после решения
//! остатка «не присвоено». Остаток решается на круге сверки или совместно (credit_rest_lock::locked) — у остановленной
//! сделки и соучастников меняется количество, а оценка в итоге дня (и «своё» в слепке просадки решения 414 —
//! drawdown_ledger.rs: остановленная = вложено + оценка) была посчитана по прежнему количеству. Здесь — тот же пересчёт,
//! что у ledger::finish для остановленной (daily::provisional): прежняя оценка сделки в книге дня заменяется новой
//! (daily::record снимает старую — записи не дублируются), начисления дня, сумма оценки и строка хронологии переносятся в
//! сделку учёта в памяти (как deal_book::keep_day — остальное, что успел поменять человек, не перетирается).
//! Пересчитываются только сделки, у которых оценка уже есть (provisional_usd) и шаг «остановлена», «ждёт решения
//! человека» или «Беру на себя» (те, что ledger::finish и ledger_load оценивают), и только если количество изменилось.
//! Кто вызывает: core/credit_rest_lock.rs (locked — своя сделка и соучастники).
//! От чего зависит: daily.rs (record), daily_value.rs (value_now, estimate), daily_commit.rs (announce), deal.rs, engine.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use super::deal::{Deal, Step};
use super::engine::{Core, lk};

/// Что делает: у сделки есть оценка в итоге дня, которую нужно держать по количеству. Принимает: сделку.
/// Возвращает: true — шаг Halted, WaitingHuman или TakenOver и оценка уже записана. Ошибок нет.
pub fn valued(d: &Deal) -> bool {
    matches!(d.step, Step::Halted | Step::WaitingHuman | Step::TakenOver)
        && d.provisional_usd.is_some()
}

/// Что делает: пересчёт оценки сделки учёта `id`, если её количество отличается от `before` (шапка). Котировка
/// (daily_value::value_now — await, цены нет — худший случай) — до записи; после неё учёт в памяти перечитывается
/// (ревью dd29ef5a): сделка всё ещё с оценкой (valued) и с тем же количеством — только тогда daily::record (как
/// daily::provisional) на свежей копии; иначе (человек закрыл, продал, взял) ничего не пишется. Затем сумма, начисления
/// дня и строки хронологии — в сделку учёта в памяти; журнал и Telegram сработавших лимитов (daily_commit::announce).
/// Принимает: ядро, ID сделки, количество до решения. Возвращает: Some(сделка учёта после пересчёта) — пересчитана
/// (сохраняет вызывающий), None — сделки нет, количество не изменилось, оценки у неё нет или сделка изменилась за
/// время котировки. Ошибок нет.
pub async fn revalue(core: &Core, id: &str, before: Decimal) -> Option<Deal> {
    let x = lk(&core.inner.deals).get(id).cloned()?;
    if x.qty == before || !valued(&x) {
        return None;
    }
    let value = Box::pin(super::daily_value::value_now(core, &x)).await;
    let mut y = {
        let m = lk(&core.inner.deals);
        let y = m.get(id)?;
        if !valued(y) || y.qty != x.qty {
            return None;
        }
        y.clone()
    };
    let (tl, db) = (y.timeline.len(), y.day_book.len());
    let est = super::daily_value::estimate(&y, value);
    let trip = Box::pin(super::daily::record(core, &mut y, est)).await;
    let d = {
        let mut m = lk(&core.inner.deals);
        let d = m.get_mut(id)?;
        d.provisional_usd = y.provisional_usd;
        d.day_book.extend(y.day_book.iter().skip(db).cloned());
        d.timeline.extend(y.timeline.iter().skip(tl).cloned());
        d.clone()
    };
    Box::pin(super::daily_commit::announce(core, &trip)).await;
    Some(d)
}

/// Что делает: пересчёт своей сделки вызывающего (locked держит её `&mut`): revalue по учёту в памяти и перенос
/// результата (оценка, начисления дня, хронология) в `deal`, чтобы следующее сохранение вызывающего его не потеряло.
/// Принимает: ядро, сделку, количество до решения. Ошибок нет: не пересчитана — `deal` без изменений.
pub async fn revalue_own(core: &Core, deal: &mut Deal, before: Decimal) {
    if deal.qty == before || !valued(deal) {
        return;
    }
    if let Some(d) = Box::pin(revalue(core, &deal.id, before)).await {
        deal.provisional_usd = d.provisional_usd;
        deal.day_book = d.day_book;
        deal.timeline = d.timeline;
    }
}
