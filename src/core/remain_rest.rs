//! Файл: core/remain_rest.rs
//! Что делает: решение 413 (седьмое ревью t198, С-1; вынесено из remain_fact.rs — размер файла) — итог сверки остатка
//! сделки, чей остаток «не присвоено» решён тем же чтением факта под замком (credit_rest::locked): монеты есть — вести
//! дальше; без монет — на счёте пусто: фантом закрывается; на счёте есть, но всё других — алерт человеку (решение
//! 405, empty). Свежий факт для второго входа подряд — как у remain_fact::reconcile_as.
//! empty — решённая без монет: пусто — фантом закрыт; есть, но всё других — алерт человеку (решение 405).
//! Кто вызывает: core/remain_fact.rs (reconcile_as), core/credit_rest_park.rs и core/credit_rest_lock.rs (empty).
//! От чего зависит: remain_alert.rs, ledger.rs, remain_others.rs, deal.rs, engine.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use super::deal::{Deal, Step};
use super::engine::Core;
use super::remain_others::Others;

/// Что делает: итог после решённого остатка (шапка). Принимает: ядро, сделку, факт, учёт других. Возвращает: как
/// remain_fact::reconcile — true вести дальше. Ошибок нет.
pub async fn after(core: &Core, deal: &mut Deal, fact: Decimal, others: &Others) -> bool {
    deal.fact_unread = false;
    if deal.qty > Decimal::ZERO {
        deal.fact_fresh = Some(Box::new((deal.step, deal.qty, deal.at.clone())));
        return true;
    }
    let why = "остатка на площадке нет — фантом учёта (решение 222, 413)";
    Box::pin(empty(core, deal, fact, others, why)).await;
    false
}

/// Что делает: решённая сделка без монет (седьмое ревью, мелкое 1): на счёте пусто — фантом закрывается; на счёте
/// есть, но всё — других (решение 405) — алерт человеку (remain_alert::all_foreign), не молчаливое закрытие.
/// Принимает: ядро, сделку, факт, учёт других, причину закрытия. Ошибок нет.
pub async fn empty(core: &Core, deal: &mut Deal, fact: Decimal, others: &Others, why: &str) {
    if fact > Decimal::ZERO {
        Box::pin(super::remain_alert::all_foreign(core, deal, fact, others)).await;
        return;
    }
    Box::pin(super::ledger::finish(core, deal, Step::Closed, why)).await
}
