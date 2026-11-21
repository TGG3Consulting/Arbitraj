//! Файл: core/unsettled.rs
//! Что делает: решение 405 (t191, ревью 3 М2: «held проходит все сделки истории под замком deals на горячем пути
//! покупки») — набор ID сделок, которые ещё что-то держат (`inner.unsettled`): сделка не закрыта (любой шаг, кроме
//! «закрыта» и «Беру на себя»), или её выручка ждёт общего возврата группы (Deal.group.wait), или у неё есть часть
//! возврата USDT, не законченная (не «вернулись» и не «перегнано вручную»). Резерв возвратов (usdt_hold.rs) и монеты
//! других сделок на счёте (remain_others.rs) смотрят только эти сделки — стоимость O(размер набора) поисков в учёте
//! вместо прохода по всей истории.
//!
//! Пополнение — при каждом сохранении сделки (ledger::save), создании (ledger::open), загрузке (ledger_load.rs) и смене
//! состояния возврата в памяти (usdt_return_state::set); снятие — там же и лениво при чтении (keep = false). Сделка вне
//! набора (закрыта, все части возврата закончены, ничего не ждёт) снова держать ничего не начинает без сохранения:
//! новые части возврата появляются только у закрытия (ledger::finish → save) и у незаконченных частей (они в наборе).
//! Блокировки: track — только `unsettled`; drop_ids — `deals`, затем `unsettled` (обратного порядка нигде нет).
//! Кто вызывает: ledger.rs (save, open), ledger_load.rs, usdt_return_state.rs (set), usdt_hold.rs, remain_others.rs.
//! От чего зависит: deal.rs, usdt_return_parts.rs, engine.rs.
//! Настройки: нет.

use super::deal::{Deal, Step};
use super::engine::{Core, lk};

/// Что делает: держит ли сделка что-то (см. шапку). Принимает: сделку. Ошибок нет.
pub fn keep(d: &Deal) -> bool {
    if !matches!(d.step, Step::Closed | Step::TakenOver) {
        return true;
    }
    if d.group.as_ref().is_some_and(|g| g.wait.is_some()) {
        return true;
    }
    super::usdt_return_parts::all(d)
        .iter()
        .any(|p| !p.finished())
}

/// Что делает: внести или убрать сделку по keep. Принимает: ядро, сделку. Ошибок нет. Замок: unsettled.
pub fn track(core: &Core, d: &Deal) {
    let mut s = lk(&core.inner.unsettled);
    if keep(d) {
        s.insert(d.id.clone());
    } else {
        s.remove(&d.id);
    }
}

/// Что делает: копия набора. Принимает: ядро. Возвращает: ID сделок. Ошибок нет. Замок: unsettled (коротко).
pub fn ids(core: &Core) -> Vec<String> {
    lk(&core.inner.unsettled).iter().cloned().collect()
}

/// Что делает: убрать сделки, которые уже ничего не держат (найдены при чтении), — с повторной проверкой под замками
/// `deals`, затем `unsettled` (сохранение могло успеть вернуть сделку в набор: ledger::save пишет учёт раньше, чем
/// track). Принимает: ядро, ID. Ошибок нет.
pub fn drop_ids(core: &Core, gone: &[String]) {
    if gone.is_empty() {
        return;
    }
    let deals = lk(&core.inner.deals);
    let mut s = lk(&core.inner.unsettled);
    for id in gone {
        if deals.get(id).is_none_or(|d| !keep(d)) {
            s.remove(id);
        }
    }
}
