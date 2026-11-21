//! Файл: core/group_notice.rs
//! Что делает: решение 394 (ревью 3 t179, Н4; записка 269, п. 12) — «Итог группы» докупки в Telegram ровно один раз.
//! Носитель общего возврата — сделка 1 с отложенным deal.closed (deal_closed::hold): итог группы — строкой её
//! deal.closed (DealGroup::lines). Носитель уже известил о своём закрытии (вложенная; закрывшаяся после общего
//! возврата) — общий возврат (coin_group_flush.rs) ставит DealGroup.total_due, и когда возврат закончен (все части
//! «вернулись», «не вернулись» или «перегнано вручную»), уходит отдельное сообщение событием deal.closed: строки
//! группы и возврат USDT с затратами по факту. Без группы — сюда не попадают.
//! Кто вызывает: coin_group_flush.rs (mark, take), usdt_return_state.rs (set), usdt_return_pieces.rs, ledger_load.rs.
//! От чего зависит: deal.rs, deal_group.rs, deal_return_cost.rs (возврат идёт), deal_text.rs (строка возврата),
//! notify/human, deps.rs.
//! Настройки: нет (событие deal.closed включается на экране уведомлений).

use crate::notify::human as h;

use super::deal::Deal;
use super::engine::Core;

/// Что делает: носитель общего возврата с итогом группы, чьё deal.closed уже ушло (нет отложенного), — «Итог группы»
/// отправить отдельно по концу возврата. Принимает: носителя (до сохранения). Ошибок нет.
pub fn mark(deal: &mut Deal) {
    if deal.closed_notice.is_some() {
        return;
    }
    if let Some(g) = deal.group.as_mut().filter(|x| x.sum.is_some()) {
        g.total_due = true;
    }
}

/// Что делает: «Итог группы» готов — возврат больше не идёт; снимает признак. Принимает: сделку (под замком учёта, до
/// сохранения). Возвращает: текст сообщения или None (не нужно, уже ушло или возврат идёт). Ошибок нет.
pub fn take(deal: &mut Deal) -> Option<String> {
    let due = deal.group.as_ref().is_some_and(|g| g.total_due);
    if !due || super::deal_return_cost::in_progress(deal) {
        return None;
    }
    if let Some(g) = deal.group.as_mut() {
        g.total_due = false;
    }
    let grp = deal.group.as_deref()?;
    let mut m = h::Msg::new(h::INFO, format!("Итог группы докупки {}", grp.id));
    for (k, v) in grp.lines(&deal.id, deal.result_usd) {
        m = m.kv(k, v);
    }
    if let Some(r) = super::deal_text::return_line(deal) {
        m = m.kv("Возврат USDT", r);
    }
    Some(m.id(&deal.id).render())
}

/// Что делает: отправляет «Итог группы» (событие deal.closed, площадка покупки). Принимает: ядро, сделку-носителя,
/// текст (take). Ошибок нет.
pub async fn send(core: &Core, deal: &Deal, text: &str) {
    core.inner
        .deps
        .notify("deal.closed", Some(&deal.direction.buy_on.0), text)
        .await;
}
