//! Файл: core/deposit_proof.rs
//! Что делает: решение 411 (задача t196, Тигран 19.11: «адрес депозита — из памяти, но умную связку: если я завтра
//! поменяю API-ключи, бот не должен взять неправильный адрес») — скорость в предпроверке, безопасность перед выводом.
//! - Предпроверка (precheck_deposit.rs ready): адрес биржи B есть в памяти адаптера (под отпечатком ключа API,
//!   gateway/addr_book.rs) и последний свежий запрос по нему не падал — шаг не ждёт; одновременно, вне критического
//!   пути (отдельная задача tokio), уходит свежий запрос адреса к бирже (режим Fresh: память не читается). Адреса в
//!   памяти нет — как раньше: предпроверка ждёт ответ биржи (это и есть свежий ответ).
//! - Свежий ответ сверяется с памятью (deposit_proof_plan.rs compare): совпал — ничего; другой — адаптер уже положил
//!   свежий в память, запись журнала core.deposit_address (Warn) и событие Telegram venue.deposit_address_changed — без
//!   значений адреса и memo. Упал — ключ помечается «не подтверждён»: следующая предпроверка ждёт ответ биржи.
//! - Подтверждение (Proof) кладётся в задачу сделки (task_local, executor.rs scope). Перевод сделки на биржу
//!   (transfers.rs go → for_transfer) берёт адрес ТОЛЬКО из него, дождавшись свежего ответа, если он ещё идёт;
//!   подтверждения нет, оно другого ключа, ключи API сменились (forget) или свежий запрос упал — свой свежий запрос
//!   перед выводом (как раньше при промахе памяти); упал и он — ошибка адреса, как раньше (вывода нет).
//! - Один запрос на ключ одновременно: вторая предпроверка того же ключа, пока запрос идёт, ждёт его итог.
//! Кто вызывает: core/precheck_deposit.rs (ready), core/transfers.rs (for_transfer), core/executor.rs (scope),
//! core/engine.rs (forget — Core::secrets_changed).
//! От чего зависит: deposit_proof_plan.rs, gateway/addr_book.rs (memory_only, fresh), gateway/urgency.rs,
//! gateway/patience.rs, шлюз (deposit_address).
//! Настройки: нет.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use tokio::sync::watch;

use crate::core::types::{Asset, VenueId};
use crate::gateway::addr_book;
use crate::gateway::contract::Venue;
use crate::storage::journal::Level;

use super::deposit_proof_plan::{self as plan, Fresh, Pick, Seen, State};
use super::engine::Core;

/// Ключ: (площадка, монета тикером площадки, сеть площадки) — тот же, что у памяти адаптера (без отпечатка: его знает
/// только адаптер; смена ключей сбрасывает всё здесь — forget).
pub type Key = (VenueId, String, String);

/// Канал итога свежего запроса: None — ещё идёт.
type Rx = watch::Receiver<Option<Fresh>>;

/// Поколение ключей API: растёт на каждой смене секретов (forget).
static GEN: AtomicU64 = AtomicU64::new(0);
/// Идущие свежие запросы: ключ → (поколение, канал итога).
static INFLIGHT: LazyLock<Mutex<HashMap<Key, (u64, Rx)>>> = LazyLock::new(Default::default);
/// Ключи, у которых последний свежий запрос упал: из памяти без ожидания не берутся.
static DOUBT: LazyLock<Mutex<HashSet<Key>>> = LazyLock::new(Default::default);

tokio::task_local! {
    /// Подтверждение адреса сделки (кладёт предпроверка, берёт перевод).
    static SLOT: RefCell<Option<Proof>>;
}

/// Подтверждение адреса: свежий запрос к бирже (идёт или пришёл).
#[derive(Clone)]
pub struct Proof {
    key: Key,
    epoch: u64,
    rx: Rx,
}

impl Proof {
    /// Что делает: ждёт итог свежего запроса. Возвращает: Fresh (задача запроса прервана — Err с причиной).
    /// Ошибок нет.
    pub async fn wait(&self) -> Fresh {
        let mut rx = self.rx.clone();
        match rx.wait_for(|v| v.is_some()).await {
            Ok(v) => (*v)
                .clone()
                .unwrap_or_else(|| Err("свежий запрос адреса прерван".into())),
            Err(_) => Err("свежий запрос адреса прерван".into()),
        }
    }
}

/// Что делает: смена секретов (разблокировка, ключ API) — подтверждения и пометки прежних ключей не используются.
/// Ошибок нет.
pub fn forget() {
    GEN.fetch_add(1, Ordering::SeqCst);
    INFLIGHT.lock().unwrap_or_else(|p| p.into_inner()).clear();
    DOUBT.lock().unwrap_or_else(|p| p.into_inner()).clear();
}

/// Что делает: задача сделки с местом для подтверждения адреса (executor.rs). Принимает: работу. Возвращает: её итог.
/// Ошибок нет.
pub async fn scope<F: Future>(work: F) -> F::Output {
    SLOT.scope(RefCell::new(None), work).await
}

/// Что делает: кладёт подтверждение в задачу сделки (вне scope — ничего). Ошибок нет.
pub fn stash(p: Proof) {
    let _ = SLOT.try_with(|s| *s.borrow_mut() = Some(p));
}

/// Что делает: забирает подтверждение задачи сделки (одно на сделку). Ошибок нет.
fn take() -> Option<Proof> {
    SLOT.try_with(|s| s.borrow_mut().take()).ok().flatten()
}

/// Что делает: адрес из памяти адаптера под текущим ключом API, без запроса к бирже. Принимает: адаптер, монету, сеть.
/// Возвращает: адрес или None. Ошибок нет.
pub async fn memory(gw: &Arc<dyn Venue>, on: &Asset, net: &str) -> Option<String> {
    addr_book::memory_only(gw.deposit_address(on, net))
        .await
        .ok()
}

/// Что делает: последний свежий запрос по ключу упал (адрес памяти не подтверждён). Ошибок нет.
pub fn doubted(key: &Key) -> bool {
    DOUBT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .contains(key)
}

/// Что делает: свежий запрос адреса — отдельной задачей (один на ключ одновременно). Принимает: ядро, адаптер, ключ,
/// монету тикером площадки, адрес из памяти (для сверки). Возвращает: Proof. Ошибок нет (итог — в Proof).
pub fn start(core: &Core, gw: Arc<dyn Venue>, key: Key, on: Asset, mem: Option<String>) -> Proof {
    let epoch = GEN.load(Ordering::SeqCst);
    let mut map = INFLIGHT.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((g, rx)) = map.get(&key)
        && *g == epoch
        && rx.borrow().is_none()
    {
        return Proof {
            key,
            epoch,
            rx: rx.clone(),
        };
    }
    let (tx, rx) = watch::channel(None);
    map.insert(key.clone(), (epoch, rx.clone()));
    drop(map);
    let (core, k, own) = (core.clone(), key.clone(), rx.clone());
    // Срочность и терпение задачи сделки — те же (task_local не наследуется задачей tokio).
    let class = crate::gateway::urgency::current();
    let pat = crate::gateway::patience::snapshot();
    let work = async move {
        let r: Fresh = addr_book::fresh(gw.deposit_address(&on, &k.2))
            .await
            .map_err(|e| e.message);
        let _ = tx.send(Some(r.clone()));
        // Замок — только в этом блоке (не держится через await).
        {
            let mut map = INFLIGHT.lock().unwrap_or_else(|p| p.into_inner());
            if map.get(&k).is_some_and(|(_, x)| x.same_channel(&own)) {
                map.remove(&k);
            }
        }
        after(&core, &k, mem.as_deref(), &r).await;
    };
    let work = crate::gateway::patience::restore(pat, work);
    tokio::spawn(crate::gateway::urgency::scope(class, work));
    Proof { key, epoch, rx }
}

/// Что делает: после свежего ответа — пометка ключа и сверка с памятью (журнал и Telegram при изменении).
/// Принимает: ядро, ключ, адрес из памяти, свежий ответ. Ошибок нет.
async fn after(core: &Core, key: &Key, mem: Option<&str>, r: &Fresh) {
    let (v, ticker, net): (&str, &str, &str) = (&key.0.0, &key.1, &key.2);
    let deps = &core.inner.deps;
    match r {
        Ok(a) => {
            DOUBT.lock().unwrap_or_else(|p| p.into_inner()).remove(key);
            if plan::compare(mem, a) == Seen::Changed {
                let text = plan::changed_text(v, ticker, net);
                deps.log(Level::Warn, "core.deposit_address", Some(v), &text, None)
                    .await;
                deps.notify("venue.deposit_address_changed", Some(v), &text)
                    .await;
            }
        }
        Err(_) => {
            DOUBT
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(key.clone());
            if mem.is_some() {
                let text = plan::failed_text(v, ticker, net);
                deps.log(Level::Warn, "core.deposit_address", Some(v), text, None)
                    .await;
            }
        }
    }
}

/// Что делает: адрес для вывода на биржу `to` (transfers.rs go) — только подтверждённый свежим ответом биржи: из
/// подтверждения этой сделки (дождавшись его) или своим свежим запросом. Принимает: ядро, адаптер получателя, монету
/// тикером получателя, сеть. Возвращает: адрес. Ошибки: текст адаптера — свежий запрос не дал адреса (как раньше).
pub async fn for_transfer(
    core: &Core,
    gw: &Arc<dyn Venue>,
    to: &VenueId,
    on: &Asset,
    net: &str,
) -> Result<String, String> {
    let key: Key = (to.clone(), on.ticker.clone(), net.to_string());
    let state = match take() {
        None => State::Absent,
        Some(p) if p.key != key || p.epoch != GEN.load(Ordering::SeqCst) => State::Stale,
        Some(p) => {
            let r = p.wait().await;
            // Ключи API сменились, пока ждали, — подтверждение прежнего ключа не годится.
            if p.epoch == GEN.load(Ordering::SeqCst) {
                State::Done(r)
            } else {
                State::Stale
            }
        }
    };
    match plan::for_withdraw(state) {
        Pick::Use(a) => Ok(a),
        Pick::Ask(_) => {
            let mem = memory(gw, on, net).await;
            start(core, gw.clone(), key, on.clone(), mem).wait().await
        }
    }
}
