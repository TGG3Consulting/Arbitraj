//! Файл: core/coin_group_left.rs
//! Что делает: решение 394 (ревью 3 t179; записка 269, п. 13) — сделка группы докупки, ушедшая с пути (застряла, ждёт
//! человека, остановлена, передана человеку), сразу помечается «была частью группы» (DealGroup.left) со строкой в
//! хронологию: дальше она отдельная сделка, её возврат USDT — свой (coin_group::hold_parts), общий возврат её не ждёт
//! (coin_group::running). Было: пометка появлялась только при закрытии не обычной продажей или после общего возврата.
//! Пометка не снимается: исполнитель и ручные действия сохраняют свою копию сделки (без пометки) — при сохранении
//! пометка из учёта переносится в сохраняемую запись (ledger::save).
//! Кто вызывает: ledger.rs (save, только у сделок с группой).
//! От чего зависит: deal.rs, engine.rs (учёт сделок).
//! Настройки: нет.

use super::deal::{Deal, Step};
use super::engine::{Core, lk};

/// Шаг вне пути сделки группы (п. 13). Ошибок нет.
fn off_path(s: Step) -> bool {
    matches!(
        s,
        Step::Stuck | Step::WaitingHuman | Step::Halted | Step::TakenOver
    )
}

/// Что делает: пометка «была частью группы» для сохраняемой сделки группы: шаг вне пути — пометить (строка в
/// хронологию); в учёте уже помечена — перенести пометку. Принимает: ядро, сохраняемую сделку. Возвращает: копию с
/// пометкой (сохранить её) или None — без изменений (нет группы, уже помечена, на пути). Ошибок нет.
pub fn mark(core: &Core, deal: &Deal) -> Option<Box<Deal>> {
    let g = deal.group.as_ref().filter(|x| !x.left)?;
    let was = lk(&core.inner.deals)
        .get(&deal.id)
        .is_some_and(|x| x.group.as_ref().is_some_and(|y| y.left));
    if !was && !off_path(deal.step) {
        return None;
    }
    let id = g.id.clone();
    let mut out = Box::new(deal.clone());
    if let Some(y) = out.group.as_mut() {
        y.left = true;
    }
    if !was {
        out.note(format!(
            "Была частью группы докупки {id}: сделка ушла с пути ({}) — дальше отдельной сделкой, свой возврат USDT (п. 13, решение 394)",
            super::deal_text::step_name(deal.step)
        ));
    }
    Some(out)
}
