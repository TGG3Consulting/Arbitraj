//! Файл: core/deposit_proof.rs
//! Что делает: решение 411, переделка (Тигран 19.11: «если адрес не изменился — зачем каждый раз спрашивать? Делать по
//! триггеру изменения») — адрес депозита биржи берётся из памяти адаптера (под отпечатком ключа API,
//! gateway/addr_book.rs) и в предпроверке, и в выводе; запрос к бирже — только по триггеру:
//!
//! (а) в памяти адреса нет под текущим ключом API (первый раз после запуска, после смены ключа) — запрос;
//! (б) смена секретов (Core::secrets_changed → forget): память прежнего ключа по новому отпечатку не находится,
//! пометки «спросить заново» забываются;
//! (в) биржа-отправитель отклонила вывод с кодом шлюза InvalidAddress (transfers.rs go → doubt) — следующий раз
//! адрес биржи-получателя запрашивается заново;
//! (г) бот зафиксировал «не дошло»: перевод дольше transfer.max_time_s (transfer_watch.rs slow) или статус депозита
//! с кодом DepositNotCredited (transfer_credit.rs) — то же.
//!
//! Ответ триггерного запроса сверяется с прежним значением памяти (deposit_proof_plan.rs compare): другой — журнал
//! core.deposit_address (Warn) и событие Telegram venue.deposit_address_changed без значений адреса и memo.
//! Фоновых задач нет: триггерный запрос идёт в задаче вызывающего.
//! Кто вызывает: core/precheck_deposit.rs (address), core/transfers.rs (address, doubt), core/transfer_watch.rs и
//! core/transfer_credit.rs (doubt), core/engine.rs (forget).
//! От чего зависит: deposit_proof_plan.rs, precheck_deposit.rs (target_on), gateway/addr_book.rs (memory_only, fresh),
//! шлюз (deposit_address).
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use crate::core::types::{Asset, VenueId};
use crate::gateway::addr_book;
use crate::gateway::contract::Venue;
use crate::gateway::errors::GatewayError;
use crate::storage::journal::Level;

use super::deposit_proof_plan::{self as plan, Seen};
use super::engine::Core;

/// Ключ: (площадка, монета тикером площадки, сеть площадки) — тот же, что у памяти адаптера, без отпечатка: его знает
/// только адаптер; пометки снимаются при смене секретов (forget).
pub type Key = (VenueId, String, String);

/// Ключи «спросить заново» (триггеры в, г) и причина — для журнала.
static DOUBT: LazyLock<Mutex<HashMap<Key, String>>> = LazyLock::new(Default::default);

/// Что делает: триггер (б) — смена секретов: пометки прежнего ключа не нужны (память прежнего ключа и так не
/// находится по новому отпечатку). Ошибок нет.
pub fn forget() {
    DOUBT.lock().unwrap_or_else(|p| p.into_inner()).clear();
}

/// Что делает: стоит ли пометка «спросить заново» на ключе. Ошибок нет.
pub fn doubted(key: &Key) -> bool {
    DOUBT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .contains_key(key)
}

/// Что делает: триггеры (в), (г) — пометить адрес монеты на площадке-получателе «спросить заново» и записать в журнал.
/// Принимает: ядро, получателя, монету (ядра), причину. Ошибок нет (сети BSC у монеты нет — пометки нет).
pub async fn doubt(core: &Core, to: &VenueId, asset: &Asset, why: &str) {
    let Ok(t) = super::precheck_deposit::target_on(core, asset, to) else {
        return;
    };
    let key: Key = (to.clone(), t.on.ticker.clone(), t.net.clone());
    let fresh = DOUBT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, why.to_string())
        .is_none();
    if fresh {
        let text = plan::doubt_text(&to.0, &t.on.ticker, &t.net, why);
        let deps = &core.inner.deps;
        deps.log(Level::Info, "core.deposit_address", Some(&to.0), text, None)
            .await;
    }
}

/// Что делает: адрес из памяти адаптера под текущим ключом API, без запроса к бирже. Принимает: адаптер, монету, сеть.
/// Возвращает: адрес или None. Ошибок нет.
async fn memory(gw: &Arc<dyn Venue>, on: &Asset, net: &str) -> Option<String> {
    addr_book::memory_only(gw.deposit_address(on, net))
        .await
        .ok()
}

/// Что делает: адрес депозита биржи — из памяти, а по триггеру (нет в памяти, пометка «спросить заново») — запросом к
/// бирже (режим Fresh) со сверкой. Принимает: ядро, адаптер биржи, ключ, монету тикером биржи.
/// Возвращает: адрес. Ошибки: GatewayError адаптера (код сохраняется — правила шлюза у вывода, transfers.rs).
pub async fn address(
    core: &Core,
    gw: &Arc<dyn Venue>,
    key: &Key,
    on: &Asset,
) -> Result<String, GatewayError> {
    let mem = memory(gw, on, &key.2).await;
    if plan::use_memory(mem.is_some(), doubted(key))
        && let Some(a) = &mem
    {
        return Ok(a.clone());
    }
    let r = addr_book::fresh(gw.deposit_address(on, &key.2)).await;
    if let Ok(a) = &r {
        after(core, key, mem.as_deref(), a).await;
    }
    r
}

/// Что делает: после триггерного ответа — снять пометку, сверить с прежней памятью (изменился — журнал и Telegram).
/// Принимает: ядро, ключ, прежний адрес памяти, свежий адрес. Ошибок нет.
async fn after(core: &Core, key: &Key, mem: Option<&str>, a: &str) {
    DOUBT.lock().unwrap_or_else(|p| p.into_inner()).remove(key);
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
