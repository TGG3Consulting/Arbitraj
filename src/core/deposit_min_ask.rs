//! Файл: core/deposit_min_ask.rs
//! Что делает: решение 387 — запрос сведений монеты у биржи-получателя СРАЗУ, когда расчёт направления упёрся в
//! неизвестный минимальный депозит B (живой факт 16–17.11.2026: после перезапуска «→ gate» 14 минут давали отказ
//! «gate: минимальный депозит неизвестен — сделки нет», 1497 отказов, пока фоновый круг адаптера не дошёл до монеты).
//! Как: контекст направления (dirctx.rs) или оценка перевода (relay_value.rs) видит у B запись сети без минимума
//! депозита → ask(): по этой бирже и монете вызывается метод шлюза deposit_networks_of (сети монеты ДЛЯ ПРИЁМА —
//! отдельно от coin_networks_of предпроверки и подсказки имён сети; ревью t172 Б1), ответ пишется в справочник сетей
//! ядра тем же кодом, что у предпроверки (networks_merge.rs), пересборка справочника монет — и следующий тик кандидата
//! считается уже с минимумом. Ядро не знает, как биржа получает минимум: у Gate — адрес депозита
//! (venues/gate/deposit_min.rs); у бирж, где минимум — поле справочника сетей, метод отвечает NotApplicable, и эту
//! биржу ядро больше не спрашивает (без хардкода списка бирж; по ней действует venue.deposit_min_absent_ok).
//! Запросы к одной бирже — строго по одному (очередь в порядке обращений); темп — у адаптера (Gate:
//! venue.deposit_info_requests_per_sec).
//! Повтор по монете (ревью Н2): минимум получен — больше не спрашиваем; ответ без минимума («временно пусто»: адрес не
//! выдан, ввод отклонён) или сбой — повтор не раньше интервала фонового опроса справочника сетей этой биржи
//! (registry_sync.rs poll_ms: venue.networks_poll_ms, не задана — venue.status_poll_ms; своего числа нет) или сразу
//! после разблокировки/смены секретов (forget — Core::secrets_changed). Не задан ни один интервал — повтор только после
//! смены секретов. Запрос, начатый до смены секретов и упавший, не откладывает повтор (поколение секретов, ревью Н4).
//! Путь расчёта (ревью Н3): проверка «площадка не даёт минимум / уже спрошено» — первой, под замком на чтение, без
//! чтения настроек; настройка читается только для повтора после сбоя.
//! Журнал: сбой — запись core.deposit_min (повтор того же текста по бирже — без записи); минимум получен — запись Info.
//! Кто вызывает: core/dirctx.rs (build_with), core/relay_value.rs (deposit_ok), core/engine.rs (forget).
//! От чего зависит: engine.rs (шлюз, журнал), registry_sync.rs (poll_ms), native.rs, networks_merge.rs.
//! Настройки: venue.networks_poll_ms / venue.status_poll_ms (через registry_sync.rs), своих нет.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::core::types::VenueId;
use crate::gateway::errors::ErrorCode;
use crate::storage::journal::Level;

use super::engine::{Core, lk, rd, wr};

/// Что известно о запросе по (биржа, монета).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Запрос стоит в очереди или идёт.
    Busy,
    /// Минимум получен — больше не спрашиваем до смены секретов.
    Answered,
    /// Ответ без минимума или сбой в этот момент — повтор не раньше интервала опроса справочника сетей биржи.
    Failed(Instant),
}

/// Итог одного запроса.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Минимум в ответе есть.
    Answered,
    /// Минимума нет или сбой — повтор по интервалу.
    Retry,
    /// Площадка не даёт минимум отдельным запросом — больше не спрашивать её.
    Unsupported,
}

/// Ключ: биржа и тикер монеты на ней.
type Key = (VenueId, String);
/// Очередь одной биржи: замок (запросы по одному) и последний записанный в журнал текст сбоя.
type Lane = Arc<tokio::sync::Mutex<Option<String>>>;

static ASKED: LazyLock<RwLock<HashMap<Key, State>>> = LazyLock::new(Default::default);
static LANES: LazyLock<Mutex<HashMap<VenueId, Lane>>> = LazyLock::new(Default::default);
/// Площадки, ответившие NotApplicable (минимум — только в справочнике сетей).
static UNSUPPORTED: LazyLock<RwLock<HashSet<VenueId>>> = LazyLock::new(Default::default);
/// Поколение секретов: +1 на каждый forget.
static GEN: AtomicU64 = AtomicU64::new(0);

/// Что делает: пора ли спрашивать (чистая логика, без сети). Принимает: прошлое состояние, сейчас, интервал повтора
/// (None — не задан). Возвращает: да — спрашивать. Ошибок нет.
pub fn due(s: Option<State>, now: Instant, every: Option<Duration>) -> bool {
    match s {
        None => true,
        Some(State::Busy) | Some(State::Answered) => false,
        Some(State::Failed(at)) => every.is_some_and(|e| now.saturating_duration_since(at) >= e),
    }
}

/// Что делает: итог запроса по ответу (чистая логика). Принимает: Ok(есть ли минимум в ответе) или код ошибки.
/// Возвращает: Outcome. Ошибок нет.
pub fn outcome(res: Result<bool, ErrorCode>) -> Outcome {
    match res {
        Ok(true) => Outcome::Answered,
        Ok(false) => Outcome::Retry,
        Err(ErrorCode::NotApplicable) => Outcome::Unsupported,
        Err(_) => Outcome::Retry,
    }
}

/// Что делает: ставит запрос сведений монеты `on` у биржи `v` в очередь этой биржи, если пора (due); иначе ничего.
/// Дёшево: площадка без отдельного источника минимума и уже спрошенная монета отсекаются под замком на чтение.
/// Вне рантайма tokio — ничего. Принимает: ядро, биржу B, тикер монеты на этой бирже. Ошибок нет.
pub fn ask(core: &Core, v: &VenueId, on: &str) {
    if rd(&UNSUPPORTED).contains(v) {
        return;
    }
    let key: Key = (v.clone(), on.to_string());
    let seen = rd(&ASKED).get(&key).copied();
    if matches!(seen, Some(State::Busy) | Some(State::Answered)) {
        return;
    }
    let every =
        || super::registry_sync::poll_ms(&core.inner.deps.settings, v).map(Duration::from_millis);
    if seen.is_some() && !due(seen, Instant::now(), every()) {
        return;
    }
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    {
        let mut m = wr(&ASKED);
        // Повторная проверка под замком на запись: другой поток мог поставить запрос между чтением и записью.
        if m.get(&key) != seen.as_ref() {
            return;
        }
        m.insert(key.clone(), State::Busy);
    }
    let lane = lk(&LANES).entry(v.clone()).or_default().clone();
    let generation = GEN.load(Ordering::SeqCst);
    let core = core.clone();
    rt.spawn(async move {
        let mut last = lane.lock().await;
        let out = one(&core, &key, &mut last).await;
        settle(key, out, generation);
    });
}

/// Что делает: записывает итог запроса. Сбой запроса, начатого до смены секретов, не откладывает повтор (ключ
/// снимается — следующий тик спросит снова). Принимает: ключ, итог, поколение секретов на момент постановки.
/// Ошибок нет.
fn settle(key: Key, out: Outcome, generation: u64) {
    let mut m = wr(&ASKED);
    match out {
        Outcome::Answered => {
            m.insert(key, State::Answered);
        }
        Outcome::Retry if GEN.load(Ordering::SeqCst) == generation => {
            m.insert(key, State::Failed(Instant::now()));
        }
        Outcome::Retry => {
            m.remove(&key);
        }
        Outcome::Unsupported => {
            m.remove(&key);
            wr(&UNSUPPORTED).insert(key.0);
        }
    }
}

/// Что делает: один запрос deposit_networks_of и запись ответа в справочник ядра. Принимает: ядро, ключ, последний
/// записанный текст сбоя этой биржи. Возвращает: итог. Ошибок нет (сбой — запись в журнал).
async fn one(core: &Core, key: &Key, last: &mut Option<String>) -> Outcome {
    let (v, on) = key;
    let deps = &core.inner.deps;
    let res = match deps.gw.get(v).await {
        Ok(g) => g.deposit_networks_of(on).await,
        Err(e) => Err(e),
    };
    match res {
        Ok(list) => {
            let list = super::net_chain::prepare(core, v, list);
            let min = list
                .iter()
                .find(|i| &i.asset.ticker == on && i.deposit_min.is_some())
                .and_then(|i| i.deposit_min);
            super::networks_merge::merge_coin(core, v, on, list);
            *last = None;
            if let Some(m) = min {
                let msg = format!(
                    "Минимум депозита {on} на {} получен запросом по монете (расчёт упёрся в неизвестный минимум): {m}",
                    v.0
                );
                deps.log(Level::Info, "core.deposit_min", Some(&v.0), msg, None)
                    .await;
            }
            outcome(Ok(min.is_some()))
        }
        Err(e) => {
            let out = outcome(Err(e.code));
            if out == Outcome::Retry && last.as_deref() != Some(e.message.as_str()) {
                let msg = format!(
                    "Минимум депозита {on} на {} не запрошен: {} — повтор не раньше интервала опроса справочника \
                     сетей биржи или после разблокировки секретов",
                    v.0, e.message
                );
                deps.log(Level::Warn, "core.deposit_min", Some(&v.0), msg, None)
                    .await;
                *last = Some(e.message);
            }
            out
        }
    }
}

/// Что делает: забыть ответы и сбои (секреты разблокированы или изменены — Core::secrets_changed): следующий расчёт с
/// неизвестным минимумом спросит снова. Идущие запросы не трогаются; их сбой после этого повтор не откладывает
/// (поколение секретов). Ошибок нет.
pub fn forget() {
    GEN.fetch_add(1, Ordering::SeqCst);
    wr(&ASKED).retain(|_, s| *s == State::Busy);
}
