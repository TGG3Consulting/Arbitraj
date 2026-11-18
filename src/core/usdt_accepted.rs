//! Файл: core/usdt_accepted.rs
//! Что делает: решение 405 (t191, ревью 4, Б2) — признак «вывод части возврата USDT принят площадкой»: площадка
//! ответила на запрос вывода созданным выводом (transfers.rs go: `created` — факт ответа API), значит деньги с неё уже
//! списаны, и резерв этой части (usdt_hold.rs) должен уменьшиться сразу, а не ждать хеша вывода (у биржи хеш приходит
//! через 11–89 с: всё это время резерв «идёт» стоял поверх уже уменьшившегося свободного, и перегон другой части —
//! другой монеты, без докупки — видел 0 и не отправлялся).
//! Запись: ID перевода (перегон «<сделка>u…»), площадка-отправитель, сумма вывода и сумма части «идёт» на момент
//! ответа (по ней запись узнаёт свою часть: кусок общего возврата закончился — часть «идёт» стала остатком, запись
//! устарела и снимается при чтении). Пишется только у перегонов USDT сделок (transfers::send_as; есть своя сделка с
//! частью «идёт» на этой площадке — usdt_hold::owner_of); ручной перевод USDT и переводы монеты — нет.
//! Снимается: вывод отклонён после принятия (WithdrawRejected в статусе — деньги вернулись на площадку) — unmark;
//! часть сменила состояние (вернулись, не вернулись, ждёт повтора — usdt_return_state::set) — forget_deal; кусок
//! закончился — устаревшая запись при чтении. В памяти: после перезапуска «идёт» становится «не вернулись»
//! (usdt_return_state::on_restart) — записи не нужны.
//! Кто вызывает: transfers.rs (mark, unmark), usdt_hold.rs (sent), usdt_return_state.rs (forget_deal).
//! От чего зависит: usdt_hold.rs (owner_of), usdt_return_parts.rs, relay_owner.rs, engine.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::VenueId;

use super::engine::{Core, lk};
use super::usdt_return_state::UsdtReturn;

/// Принятый площадкой вывод части.
#[derive(Debug, Clone, PartialEq)]
pub struct Accepted {
    /// Площадка-отправитель.
    pub from: VenueId,
    /// Сумма вывода (списана с площадки).
    pub qty: Decimal,
    /// Сумма части «идёт» на момент ответа площадки.
    pub part: Decimal,
}

/// Сумма части «идёт» сделки на площадке. Ошибок нет.
fn pending_of(core: &Core, deal: &str, from: &VenueId) -> Option<Decimal> {
    let deals = lk(&core.inner.deals);
    let d = deals.get(deal)?;
    super::usdt_return_parts::all(d)
        .into_iter()
        .find_map(|p| match p {
            UsdtReturn::Pending {
                amount, from: f, ..
            } if f == from => Some(*amount),
            _ => None,
        })
}

/// Что делает: площадка приняла вывод перегона `relay` (см. шапку). Принимает: ядро, ID перевода, площадку, сумму
/// вывода. Ошибок нет: перевод не перегон части «идёт» — ничего.
pub fn mark(core: &Core, relay: &str, from: &VenueId, qty: Decimal) {
    let Some(deal) = super::usdt_hold::owner_of(core, relay) else {
        return;
    };
    let Some(part) = pending_of(core, &deal, from) else {
        return;
    };
    let a = Accepted {
        from: from.clone(),
        qty,
        part,
    };
    lk(&core.inner.usdt_accepted).insert(relay.to_string(), a);
    core.inner.coin_wake.notify_waiters();
}

/// Что делает: вывод отклонён после принятия — деньги снова на площадке. Принимает: ядро, ID перевода. Ошибок нет.
pub fn unmark(core: &Core, relay: &str) {
    lk(&core.inner.usdt_accepted).remove(relay);
}

/// Что делает: часть возврата сделки сменила состояние — её записи больше не нужны. Принимает: ядро, ID сделки.
/// Ошибок нет.
pub fn forget_deal(core: &Core, deal: &str) {
    lk(&core.inner.usdt_accepted).retain(|relay, _| !super::relay_owner::is_relay(deal, relay));
}

/// Что делает: сколько уже списано площадкой по части «идёт» сделки (принятые выводы её перегонов с этой площадки и
/// той же суммой части; устаревшие записи снимаются). Принимает: ядро, сделку, площадку, текущую сумму части.
/// Возвращает: (сумма, ID перегонов — у них хеш-признак usdt_hold не считается второй раз). Ошибок нет.
pub fn sent(core: &Core, deal: &str, from: &VenueId, part: Decimal) -> (Decimal, Vec<String>) {
    let mut m = lk(&core.inner.usdt_accepted);
    m.retain(|relay, a| {
        !(super::relay_owner::is_relay(deal, relay) && a.from == *from && a.part != part)
    });
    let mut sum = Decimal::ZERO;
    let mut ids = vec![];
    for (relay, a) in m.iter() {
        if super::relay_owner::is_relay(deal, relay) && a.from == *from {
            sum += a.qty;
            ids.push(relay.clone());
        }
    }
    (sum, ids)
}
