//! Файл: core/remain_rest.rs
//! Что делает: решение 413 (седьмое ревью t198, С-1; вынесено из remain_fact.rs — размер файла) — итог сверки остатка
//! сделки, чей остаток «не присвоено» решён тем же чтением факта под замком (credit_rest::locked): монеты есть — вести
//! дальше; без монет — на счёте пусто: фантом закрывается; на счёте есть, но всё других — алерт человеку (решение
//! 405, credit_rest_park::empty). Свежий факт для второго входа подряд — как у remain_fact::reconcile_as.
//! Кто вызывает: core/remain_fact.rs (reconcile_as).
//! От чего зависит: credit_rest_park.rs (empty), remain_others.rs, deal.rs, engine.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use super::deal::Deal;
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
    Box::pin(super::credit_rest_park::empty(
        core, deal, fact, others, why,
    ))
    .await;
    false
}
