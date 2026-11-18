//! Файл: core/usdt_accepted.rs
//! Что делает: решение 405 (t191, ревью 4 Б2, ревью 5 Н1) — признак «вывод части возврата USDT принят площадкой»:
//! площадка ответила на запрос вывода созданным выводом (transfers.rs go: `created` — факт ответа API), значит деньги с
//! неё уже списаны, и резерв этой части (usdt_hold.rs) уменьшается сразу, а не ждёт хеша вывода (у биржи хеш приходит
//! через 11–89 с: всё это время резерв «идёт» стоял поверх уже уменьшившегося свободного, и перегон другой части —
//! другой монеты, без докупки — видел 0 и не отправлялся).
//! Запись — в учёте сделки (Deal.usdt_accepted, сохраняется ledger::save): ID перевода, площадка-отправитель, сумма
//! вывода и сумма части «идёт» на момент ответа (по ней запись узнаёт свою часть). Ревью 5, Н1: после перезапуска
//! «идёт» становится «не вернулись» на ту же сумму (usdt_return_state::on_restart) — запись остаётся, и резерв такой
//! части тоже уменьшается на принятое (деньги ушли); «не вернулись» без принятого вывода — в резерве целиком (решение
//! Тиграна 18.11). Пишется только у перегонов USDT сделок (transfers::send_as; есть своя сделка с частью «идёт» на
//! этой площадке — usdt_hold::owner_of); ручной перевод USDT и переводы монеты — нет.
//! Снимается: вывод отклонён после принятия (WithdrawRejected в статусе — деньги вернулись) — unmark; часть закончена
//! («вернулись», «перегнано вручную» — usdt_return_state::set → forget_finished); кусок закончился (сумма части стала
//! другой) — при следующей записи (mark) устаревшие снимаются, при чтении не считаются.
//! Кто вызывает: transfers.rs (mark, unmark), usdt_hold.rs (sent_of), usdt_return_state.rs (forget_finished).
//! От чего зависит: usdt_hold.rs (owner_of), usdt_return_parts.rs, relay_owner.rs, ledger.rs (save), engine.rs.
//! Настройки: нет.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::VenueId;

use super::deal::Deal;
use super::engine::{Core, lk};
use super::usdt_return_state::UsdtReturn;

/// Принятый площадкой вывод части.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Accepted {
    /// ID перевода (перегон «<сделка>u…»).
    pub relay: String,
    /// Площадка-отправитель.
    pub from: VenueId,
    /// Сумма вывода (списана с площадки).
    pub qty: Decimal,
    /// Сумма части «идёт» на момент ответа площадки.
    pub part: Decimal,
}

/// Что делает: площадка приняла вывод перегона `relay` (см. шапку) — запись в учёт сделки и сохранение. Принимает:
/// ядро, ID перевода, площадку, сумму вывода. Ошибок нет: перевод не перегон части «идёт» — ничего.
pub async fn mark(core: &Core, relay: &str, from: &VenueId, qty: Decimal) {
    let Some(deal) = super::usdt_hold::owner_of(core, relay) else {
        return;
    };
    let rec = {
        let mut deals = lk(&core.inner.deals);
        let Some(d) = deals.get_mut(&deal) else {
            return;
        };
        let Some(UsdtReturn::Pending { amount, .. }) =
            super::usdt_return_parts::get(d, Some(from)).cloned()
        else {
            return;
        };
        // Устаревшие записи этой площадки (кусок закончился — часть стала другой) — снять.
        d.usdt_accepted
            .retain(|a| a.from != *from || a.part == amount);
        d.usdt_accepted.retain(|a| a.relay != relay);
        d.usdt_accepted.push(Accepted {
            relay: relay.to_string(),
            from: from.clone(),
            qty,
            part: amount,
        });
        d.clone()
    };
    super::ledger::save(core, &rec).await;
}

/// Что делает: вывод отклонён после принятия — деньги снова на площадке: запись снимается. Принимает: ядро, ID
/// перевода. Ошибок нет.
pub async fn unmark(core: &Core, relay: &str) {
    let Some(deal) = super::usdt_hold::owner_of(core, relay) else {
        return;
    };
    let rec = {
        let mut deals = lk(&core.inner.deals);
        let Some(d) = deals.get_mut(&deal) else {
            return;
        };
        let before = d.usdt_accepted.len();
        d.usdt_accepted.retain(|a| a.relay != relay);
        if d.usdt_accepted.len() == before {
            return;
        }
        d.clone()
    };
    super::ledger::save(core, &rec).await;
}

/// Что делает: часть закончена («вернулись», «перегнано вручную») — записи этой площадки не нужны (вызывается под
/// замком учёта в usdt_return_state::set, перед сохранением). Принимает: сделку, площадку части. Ошибок нет.
pub fn forget_finished(d: &mut Deal, from: &VenueId) {
    d.usdt_accepted.retain(|a| a.from != *from);
}

/// Что делает: сколько уже списано площадкой по части сделки (принятые выводы её перегонов с этой площадки при той же
/// сумме части — «идёт» или ставшая после перезапуска «не вернулись»). Принимает: сделку, площадку, сумму части.
/// Возвращает: (сумма, ID перегонов — у них хеш-признак usdt_hold не считается второй раз). Ошибок нет.
pub fn sent_of(d: &Deal, from: &VenueId, part: Decimal) -> (Decimal, Vec<String>) {
    let mut sum = Decimal::ZERO;
    let mut ids = vec![];
    for a in d
        .usdt_accepted
        .iter()
        .filter(|a| a.from == *from && a.part == part)
    {
        sum += a.qty;
        ids.push(a.relay.clone());
    }
    (sum, ids)
}
