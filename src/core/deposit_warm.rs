//! Файл: core/deposit_warm.rs
//! Что делает: решение 316 — фоновый прогрев адресов депозита бирж, чтобы первая сделка после перезапуска не ждала
//! запрос адреса на горячем пути предпроверки (живой факт 16.11.2026, SHOPINX bsc_dex → bingx: «адрес депозита bingx
//! 214 мс» — бот перезапущен за 6 минут до сделки, адреса бирж кэшируются только в памяти адаптеров).
//! Как: по толчку (запуск, новый справочник монет, разблокировка/смена секретов, изменение настроек прогрева) для
//! каждой включённой биржи с venue.deposit_warm = да берутся направления справочника, где биржа — B (по одному на
//! монету), и по очереди, с паузой venue.deposit_warm_gap_ms между запросами, вызывается ТОТ ЖЕ метод шлюза
//! deposit_address с теми же монетой и сетью, что у предпроверки (precheck_deposit.rs::target) — адаптер кладёт адрес
//! в свой кэш, и предпроверка сделки берёт его из памяти. Запросы к бирже — того же вида, что делает предпроверка
//! (тот же метод шлюза, та же монета и сеть); других запросов прогрев не делает. Адрес, которого нет, прогрев не
//! создаёт (gateway/addr_claim.rs; разрешить — venue.deposit_warm_create = да). Адреса на диск не пишутся.
//! Прогрев уступает сделке (доработка по ревью): пока биржа занята сделкой (deposit_warm_busy.rs), запросов к ней нет —
//! проверка перед каждым запросом; уступившая биржа продолжает проход, когда освободится: повтор проверки не чаще её
//! venue.deposit_warm_gap_ms (своего числа нет) или со следующим толчком.
//! Проход одной биржи — deposit_warm_pass.rs; что запоминается и когда проход прекращается — deposit_warm_plan.rs.
//! Решение 411 (t196): адаптеры держат адреса под отпечатком ключа API (gateway/addr_book.rs) — после смены секретов
//! прогрев забывает и полученные (kick_account) и проверяет память без запроса; запросы — только по монетам, адреса
//! которых под текущим ключом в памяти нет. Предпроверка берёт прогретый адрес без ожидания, но перевод — только после
//! свежего ответа биржи в сделке (core/deposit_proof.rs).
//! Кто вызывает: core/engine.rs (run — при запуске; kick_account — Core::secrets_changed), core/rebuild.rs (kick — новый
//! справочник монет).
//! От чего зависит: engine.rs, deposit_warm_pass.rs, registry_sync.rs (enabled), шлюз (вид площадки).
//! Настройки: venue.deposit_warm (здесь); venue.deposit_warm_gap_ms, venue.deposit_warm_create — deposit_warm_pass.rs.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};

use futures_util::future::join_all;
use tokio::sync::Notify;

use crate::core::types::{Direction, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::deposit_warm_pass::{VenueState, venue_pass};
use super::engine::{Core, rd};

/// Толчок прохода прогрева (разрешение копится — толчки во время прохода сливаются в один следующий).
static KICK: LazyLock<Notify> = LazyLock::new(Notify::new);
/// Перед следующим проходом забыть отказы по монетам (секреты или настройки прогрева изменились).
static RETRY_FAILED: AtomicBool = AtomicBool::new(false);
/// Решение 411: перед следующим проходом забыть и полученные (секреты изменились — память адаптера под новым ключом API
/// своя; полученное под прежним ключом проверяется заново, без запроса, если адрес в памяти есть).
static FORGET_ALL: AtomicBool = AtomicBool::new(false);

const ON: &str = "venue.deposit_warm";

/// Что делает: толкает проход прогрева. Принимает: retry_failed — заново запросить монеты, которые отказали.
/// Ошибок нет.
pub fn kick(retry_failed: bool) {
    if retry_failed {
        RETRY_FAILED.store(true, Ordering::SeqCst);
    }
    KICK.notify_one();
}

/// Что делает: решение 411 — толчок после смены секретов: забыть всё (и полученные, и отказы) и пройти заново; монета,
/// адрес которой уже в памяти адаптера под текущим ключом API, запроса к бирже не делает (deposit_warm_pass.rs).
/// Ошибок нет.
pub fn kick_account() {
    FORGET_ALL.store(true, Ordering::SeqCst);
    kick(true);
}

/// Задача прогрева: первый проход — сразу, дальше — по толчку или (только уступившие сделке биржи) по их паузе.
/// Работает, пока жива система. Ошибок не возвращает.
pub async fn run(core: Core) {
    // Изменение настроек прогрева (venue.deposit_warm*: включили, задали паузу, разрешили создание) — толчок с
    // повтором отказов.
    let mut w = core.inner.deps.watch(&[ON]);
    tokio::spawn(async move {
        while w.next().await.is_some() {
            kick(true);
        }
    });
    let mut states: HashMap<VenueId, VenueState> = HashMap::new();
    kick(false);
    loop {
        // None — толчок (все биржи); Some — повтор только уступивших сделке.
        let only = wake(&states).await;
        if RETRY_FAILED.swap(false, Ordering::SeqCst) {
            states.values_mut().for_each(|s| s.memo.forget_failed());
        }
        if FORGET_ALL.swap(false, Ordering::SeqCst) {
            states.values_mut().for_each(|s| s.memo.forget_all());
        }
        let venues: Vec<VenueId> = rd(&core.inner.kinds)
            .iter()
            .filter(|(v, k)| *k == VenueKind::Cex && only.as_ref().is_none_or(|o| o.contains(v)))
            .filter(|(v, _)| on(&core, v))
            .map(|(v, _)| v.clone())
            .collect();
        // Выключенная биржа больше не ждёт повтора.
        for (v, s) in states.iter_mut() {
            if !venues.contains(v) && only.as_ref().is_none_or(|o| o.contains(v)) {
                s.wait = None;
            }
        }
        if venues.is_empty() {
            continue;
        }
        // Снимок справочника (Arc, без блокировки на время прохода); направления только торгуемые (registry.rs).
        let book = core.coins();
        let dirs: Arc<Vec<Direction>> =
            Arc::new(book.iter().flat_map(|c| c.directions()).collect());
        let jobs = venues.into_iter().map(|v| {
            let s = states.remove(&v).unwrap_or_default();
            let core = core.clone();
            let dirs = dirs.clone();
            async move {
                let s = venue_pass(&core, &v, &dirs, s).await;
                (v, s)
            }
        });
        for (v, s) in join_all(jobs).await {
            states.insert(v, s);
        }
    }
}

/// Что делает: ждёт следующего прохода — толчка или, если есть биржи, уступившие сделке, самой короткой из их пауз
/// venue.deposit_warm_gap_ms. Принимает: память бирж. Возвращает: None — толчок; Some — биржи для повтора.
/// Ошибок нет.
async fn wake(states: &HashMap<VenueId, VenueState>) -> Option<HashSet<VenueId>> {
    let Some(gap) = states.values().filter_map(|s| s.wait).min() else {
        KICK.notified().await;
        return None;
    };
    tokio::select! {
        _ = KICK.notified() => None,
        _ = tokio::time::sleep(gap) => {
            let waiting = states.iter().filter(|(_, s)| s.wait.is_some());
            Some(waiting.map(|(v, _)| v.clone()).collect())
        }
    }
}

/// Включён ли прогрев у биржи: площадка включена (venue.enabled) и venue.deposit_warm = да. Не задано — нет.
pub(super) fn on(core: &Core, v: &VenueId) -> bool {
    let at = At {
        venue: Some(&v.0),
        coin: None,
    };
    super::registry_sync::enabled(core, v) && core.inner.deps.settings.flag(ON, at).unwrap_or(false)
}
