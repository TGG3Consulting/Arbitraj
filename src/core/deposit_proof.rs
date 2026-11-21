//! Файл: core/deposit_proof.rs
//! Что делает: решение 411, переделка (Тигран 19.11: «если адрес не изменился — зачем каждый раз спрашивать? Делать по
//! триггеру изменения») — адрес депозита биржи берётся из памяти адаптера (под отпечатком ключа API,
//! gateway/addr_book.rs) и в предпроверке, и в выводе; запрос к бирже — только по триггеру.
//!
//! Триггеры. (а) В памяти адреса нет под текущим ключом API (первый раз после запуска, после смены ключа) — запрос.
//! (б) Смена секретов: память прежнего ключа по новому отпечатку не находится сама; пометки «спросить заново» при
//! этом не стираются (повторное ревью С1: смена любого секрета не должна терять пометку другой биржи). (в) Биржа-
//! отправитель отклонила вывод с кодом шлюза InvalidAddress (transfers.rs go → doubt). (г) Перевод дольше
//! transfer.max_time_s (transfer_watch.rs slow → doubt). Пометка — только у биржи-получателя (у DEX адрес — кошелёк).
//!
//! Ответ триггерного запроса сверяется с прежним значением памяти (deposit_proof_plan.rs compare), если секреты между
//! чтением памяти и ответом не менялись (addr_book::key_epoch): другой — журнал core.deposit_address (Warn) и событие
//! Telegram venue.deposit_address_changed без значений адреса и memo. Пометка снимается удачным ответом, только если
//! запрос начат после неё (номер пометки). Фоновых задач нет: запрос идёт в задаче вызывающего.
//!
//! Кто вызывает: core/precheck_deposit.rs (address), core/transfers.rs (address, doubt), core/transfer_watch.rs
//! (doubt).
//! От чего зависит: deposit_proof_plan.rs, precheck_deposit.rs (target_on), trade_ops.rs (вид площадки),
//! gateway/addr_book.rs (memory_only, fresh, is_miss, key_epoch), шлюз (deposit_address).
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use crate::core::types::{Asset, VenueId};
use crate::gateway::addr_book;
use crate::gateway::contract::{Venue, VenueKind};
use crate::gateway::errors::GatewayError;
use crate::storage::journal::Level;

use super::deposit_proof_plan::{self as plan, Seen};
use super::engine::Core;

/// Ключ: (площадка, монета тикером площадки, сеть площадки) — тот же, что у памяти адаптера (без отпечатка: его знает
/// только адаптер).
pub type Key = (VenueId, String, String);

/// Номер последней пометки (растёт на каждую).
static STAMP: AtomicU64 = AtomicU64::new(0);
/// Ключи «спросить заново» (триггеры в, г): номер пометки.
static DOUBT: LazyLock<Mutex<HashMap<Key, u64>>> = LazyLock::new(Default::default);

/// Что делает: стоит ли пометка «спросить заново» на ключе. Ошибок нет.
pub fn doubted(key: &Key) -> bool {
    DOUBT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .contains_key(key)
}

/// Что делает: триггеры (в), (г) — пометить адрес монеты на бирже-получателе «спросить заново» и записать в журнал.
/// Принимает: ядро, получателя, монету (ядра), причину. Ошибок нет (получатель не биржа или сети BSC у монеты нет —
/// пометки нет).
pub async fn doubt(core: &Core, to: &VenueId, asset: &Asset, why: &str) {
    if super::trade_ops::kind(core, to) != Ok(VenueKind::Cex) {
        return;
    }
    let Ok(t) = super::precheck_deposit::target_on(core, asset, to) else {
        return;
    };
    let key: Key = (to.clone(), t.on.ticker.clone(), t.net.clone());
    let n = STAMP.fetch_add(1, Ordering::SeqCst) + 1;
    let first = DOUBT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, n)
        .is_none();
    if first {
        let text = plan::doubt_text(&to.0, &t.on.ticker, &t.net, why);
        let deps = &core.inner.deps;
        deps.log(Level::Info, "core.deposit_address", Some(&to.0), text, None)
            .await;
    }
}

/// Что делает: адрес депозита биржи — из памяти, а по триггеру (нет в памяти, пометка «спросить заново») — запросом к
/// бирже (режим Fresh) со сверкой. Принимает: ядро, адаптер биржи, ключ, монету тикером биржи.
/// Возвращает: адрес. Ошибки: GatewayError адаптера (код сохраняется); ошибка памяти, не «нет в памяти» (например
/// адрес зачисляет не на торговый счёт), без пометки — как есть, без запроса (повторное ревью С2).
pub async fn address(
    core: &Core,
    gw: &Arc<dyn Venue>,
    key: &Key,
    on: &Asset,
) -> Result<String, GatewayError> {
    let epoch = addr_book::key_epoch();
    let started = STAMP.load(Ordering::SeqCst);
    let doubt = doubted(key);
    let mem = match addr_book::memory_only(gw.deposit_address(on, &key.2)).await {
        Ok(a) => Some(a),
        Err(e) if addr_book::is_miss(&e) || doubt => None,
        Err(e) => return Err(e),
    };
    if plan::use_memory(mem.is_some(), doubt)
        && let Some(a) = &mem
    {
        return Ok(a.clone());
    }
    let r = addr_book::fresh(gw.deposit_address(on, &key.2)).await;
    if let Ok(a) = &r {
        // Секреты менялись за время запроса — память и ответ могут быть разных аккаунтов: не сверяем.
        let mem = mem.filter(|_| addr_book::key_epoch() == epoch);
        after(core, key, started, mem.as_deref(), a).await;
    }
    r
}

/// Что делает: после ответа триггерного запроса — снять пометку (если она поставлена до начала запроса) и сверить с
/// прежней памятью (изменился — журнал и Telegram). Принимает: ядро, ключ, номер пометок на начало запроса, прежний
/// адрес памяти (None — не было или не сверяем), адрес из ответа. Ошибок нет.
async fn after(core: &Core, key: &Key, started: u64, mem: Option<&str>, a: &str) {
    {
        let mut d = DOUBT.lock().unwrap_or_else(|p| p.into_inner());
        if d.get(key).is_some_and(|n| plan::clears(*n, started)) {
            d.remove(key);
        }
    }
    if plan::compare(mem, a) != Seen::Changed {
        return;
    }
    let (v, ticker, net): (&str, &str, &str) = (&key.0.0, &key.1, &key.2);
    let text = plan::changed_text(v, ticker, net);
    let deps = &core.inner.deps;
    deps.log(Level::Warn, "core.deposit_address", Some(v), &text, None)
        .await;
    deps.notify("venue.deposit_address_changed", Some(v), &text)
        .await;
}
