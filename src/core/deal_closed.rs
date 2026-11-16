//! Файл: core/deal_closed.rs
//! Что делает: уведомление deal.closed о закрытии сделки (вынесено из ledger.rs). Текст — deal_text.rs (решение 164),
//! строки «без потолка» — deal_ceiling.rs (решение 216), чат по знаку факта — notify/routes.rs (решение 176).
//! Решение 236: закрытая сделка, у которой возврат выручки USDT ещё идёт, уведомление не шлёт сразу — причина
//! закрытия сохраняется в Deal.closed_notice (переживает перезапуск), и deal.closed уходит, когда возврат закончен
//! (все части «вернулись», «не вернулись» или «перегнано вручную»): «Факт» — уже с затратами возврата
//! (deal_return_cost.rs), чат «Сделки» / «Неудачные» — по этому факту.
//! Решение 340 (Тигран 16.11): закрытие не ждёт ни Telegram, ни чтения «без потолка» — задание в очередь уведомлений
//! (Notifier::later, notify/queue.rs, по порядку с прочими уведомлениями сделки): текст итога — из копии сделки на
//! момент закрытия, карточка — оттуда же (deal_card.rs), строки «без потолка» — читаются в задании (расчёт к этому
//! моменту только полнее).
//! Кто вызывает: ledger.rs (finish), usdt_return_state.rs (set), ledger_load.rs (после перезапуска).
//! От чего зависит: deal.rs, deal_text.rs, deal_ceiling.rs, deal_card.rs, deal_return_cost.rs, deps.rs (уведомления),
//! notify/dispatch_queue.rs (later), notify/dispatch_role.rs (tagged_now), notify/alert_card.rs (Snap).
//! Настройки: нет (событие deal.closed включается на экране уведомлений).

use super::deal::{Deal, Step};
use super::engine::Core;

/// Что делает: отправляет deal.closed — заданием очереди уведомлений (решение 340: вызывающий не ждёт). Принимает:
/// ядро, сделку (с шагом и итогом), причину закрытия. Ошибок нет.
pub async fn send(core: &Core, deal: &Deal, why: &str) {
    // Решение 176: закрытая плюсовая — ещё в чат «Сделки», минусовая — в «Неудачные» (notify/routes.rs).
    let tag = match super::deal_text::outcome(deal) {
        Some(true) => crate::notify::routes::Tag::Profit,
        Some(false) => crate::notify::routes::Tag::Loss,
        None => crate::notify::routes::Tag::Plain,
    };
    // Решение 340: копия сделки и карточка — на момент закрытия.
    let snap = crate::notify::alert_card::Snap::of(&deal.id, Some(super::deal_card::card(deal)));
    let n = core.inner.deps.notifier.clone();
    let v = deal.direction.buy_on.0.clone();
    let (c, d, w, nn, vv) = (
        core.clone(),
        deal.clone(),
        why.to_string(),
        n.clone(),
        v.clone(),
    );
    let job = async move {
        // Решение 216: у закрытой — строки расчёта «без потолка» (одно чтение SQLite по id сделки).
        let ceil = super::deal_ceiling::of_closed(&c, &d).await;
        let tg = super::deal_text::closed(&d, &w, ceil.as_ref());
        nn.tagged_now("deal.closed", Some(&vv), &tg, tag).await;
    };
    n.later("deal.closed", Some(&v), snap, job);
}

/// Что делает: решение 236 — откладывает deal.closed закрытой сделки, пока возврат USDT идёт (причина — в
/// Deal.closed_notice, строка в хронологию). Принимает: сделку (после подготовки возврата в ledger::finish), шаг, причину.
/// Возвращает: true — отложено (сейчас не слать). Ошибок нет.
pub fn hold(deal: &mut Deal, step: Step, why: &str) -> bool {
    if step != Step::Closed || !super::deal_return_cost::in_progress(deal) {
        return false;
    }
    deal.closed_notice = Some(why.to_string());
    deal.note("Уведомление о закрытии — после возврата USDT: факт с его затратами (решение 236)");
    true
}

/// Что делает: отложенное уведомление готово — возврат больше не идёт; забирает причину из сделки.
/// Принимает: сделку. Возвращает: причину закрытия (слать deal.closed) или None. Ошибок нет.
pub fn take_ready(deal: &mut Deal) -> Option<String> {
    if super::deal_return_cost::in_progress(deal) {
        return None;
    }
    deal.closed_notice.take()
}
