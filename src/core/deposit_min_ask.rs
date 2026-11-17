//! Файл: core/deposit_min_ask.rs
//! Что делает: решение 387 — запрос сведений монеты у биржи-получателя СРАЗУ, когда расчёт направления упёрся в
//! неизвестный минимальный депозит B (живой факт 16–17.11.2026: после перезапуска «→ gate» 14 минут давали отказ
//! «gate: минимальный депозит неизвестен — сделки нет», 1497 отказов, пока фоновый круг адаптера не дошёл до монеты).
//! Как: контекст направления (dirctx.rs) или оценка перевода (relay_value.rs) видит у B запись сети без минимума
//! депозита → ask(): по этой бирже и монете вызывается ТОТ ЖЕ метод шлюза coin_networks_of, что у предпроверки
//! (precheck.rs transfer_open), ответ пишется в справочник сетей ядра тем же кодом (networks_merge.rs), пересборка
//! справочника монет — и следующий тик кандидата считается уже с минимумом (если биржа его дала). Ядро не знает, как
//! биржа получает минимум: это дело адаптера (у Gate — адрес депозита, venues/gate/deposit_min.rs).
//! Запросы к одной бирже — строго по одному (очередь в порядке обращений): всплеск кандидатов после перезапуска не
//! превращается во всплеск параллельных запросов к бирже; к разным биржам — независимо.
//! Повтор по монете: ответ получен (минимум есть или биржа его не дала) — больше не спрашиваем (у бирж, где минимума в
//! API нет, сделок по монете и так нет — без повторных запросов); сбой (секреты закрыты, связь, лимит) — повтор не
//! раньше интервала фонового опроса справочника сетей этой биржи (registry_sync.rs poll_ms: venue.networks_poll_ms,
//! не задана — venue.status_poll_ms; своего числа нет) или сразу после разблокировки/смены секретов (forget —
//! Core::secrets_changed). Не задан ни один интервал — повтор только после смены секретов.
//! Журнал: сбой — запись core.deposit_min (повтор того же текста по бирже — без записи); минимум получен — запись Info.
//! Кто вызывает: core/dirctx.rs (build_with), core/relay_value.rs (deposit_ok), core/engine.rs (forget).
//! От чего зависит: engine.rs (шлюз, журнал), registry_sync.rs (poll_ms), native.rs, networks_merge.rs.
//! Настройки: venue.networks_poll_ms / venue.status_poll_ms (через registry_sync.rs), своих нет.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::core::types::VenueId;
use crate::storage::journal::Level;

use super::engine::{Core, lk};

/// Что известно о запросе по (биржа, монета).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Запрос стоит в очереди или идёт.
    Busy,
    /// Биржа ответила (минимум в ответе есть или нет) — больше не спрашиваем до смены секретов.
    Answered,
    /// Сбой запроса в этот момент — повтор не раньше интервала опроса справочника сетей биржи.
    Failed(Instant),
}

/// Ключ: биржа и тикер монеты на ней.
type Key = (VenueId, String);
/// Очередь одной биржи: замок (запросы по одному) и последний записанный в журнал текст сбоя.
type Lane = Arc<tokio::sync::Mutex<Option<String>>>;

static ASKED: LazyLock<Mutex<HashMap<Key, State>>> = LazyLock::new(Default::default);
static LANES: LazyLock<Mutex<HashMap<VenueId, Lane>>> = LazyLock::new(Default::default);

/// Что делает: пора ли спрашивать (чистая логика, без сети). Принимает: прошлое состояние, сейчас, интервал повтора
/// после сбоя (None — не задан). Возвращает: да — спрашивать. Ошибок нет.
pub fn due(s: Option<State>, now: Instant, every: Option<Duration>) -> bool {
    match s {
        None => true,
        Some(State::Busy) | Some(State::Answered) => false,
        Some(State::Failed(at)) => every.is_some_and(|e| now.saturating_duration_since(at) >= e),
    }
}

/// Что делает: ставит запрос сведений монеты `on` у биржи `v` в очередь этой биржи, если пора (due); иначе ничего.
/// Дёшево (одна блокировка карты) — вызывается на каждом расчёте с неизвестным минимумом. Вне рантайма tokio — ничего.
/// Принимает: ядро, биржу B, тикер монеты на этой бирже. Ошибок нет (сбой — в журнал из задачи запроса).
pub fn ask(core: &Core, v: &VenueId, on: &str) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let every =
        super::registry_sync::poll_ms(&core.inner.deps.settings, v).map(Duration::from_millis);
    let key: Key = (v.clone(), on.to_string());
    {
        let mut m = lk(&*ASKED);
        if !due(m.get(&key).copied(), Instant::now(), every) {
            return;
        }
        m.insert(key.clone(), State::Busy);
    }
    let lane = lk(&*LANES).entry(v.clone()).or_default().clone();
    let core = core.clone();
    rt.spawn(async move {
        let mut last = lane.lock().await;
        let st = one(&core, &key, &mut last).await;
        lk(&*ASKED).insert(key, st);
    });
}

/// Что делает: один запрос coin_networks_of и запись ответа в справочник ядра. Принимает: ядро, ключ, последний
/// записанный текст сбоя этой биржи. Возвращает: новое состояние ключа. Ошибок нет (сбой — запись в журнал).
async fn one(core: &Core, key: &Key, last: &mut Option<String>) -> State {
    let (v, on) = key;
    let deps = &core.inner.deps;
    let res = match deps.gw.get(v).await {
        Ok(g) => g.coin_networks_of(on).await,
        Err(e) => Err(e),
    };
    match res {
        Ok(list) => {
            let list = super::native::map(core, v, list);
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
            State::Answered
        }
        Err(e) => {
            if last.as_deref() != Some(e.message.as_str()) {
                let msg = format!(
                    "Минимум депозита {on} на {} не запрошен: {} — повтор не раньше интервала опроса справочника \
                     сетей биржи или после разблокировки секретов",
                    v.0, e.message
                );
                deps.log(Level::Warn, "core.deposit_min", Some(&v.0), msg, None)
                    .await;
                *last = Some(e.message);
            }
            State::Failed(Instant::now())
        }
    }
}

/// Что делает: забыть ответы и сбои (секреты разблокированы или изменены — Core::secrets_changed): следующий расчёт с
/// неизвестным минимумом спросит снова. Идущие запросы не трогаются. Ошибок нет.
pub fn forget() {
    lk(&*ASKED).retain(|_, s| *s == State::Busy);
}
