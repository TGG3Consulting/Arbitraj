//! Файл: core/deposit_warm.rs
//! Что делает: решение 316 — фоновый прогрев адресов депозита бирж, чтобы первая сделка после перезапуска не ждала
//! запрос адреса на горячем пути предпроверки (живой факт 16.11.2026, SHOPINX bsc_dex → bingx: «адрес депозита bingx
//! 214 мс» — бот перезапущен за 6 минут до сделки, адреса бирж кэшируются только в памяти адаптеров).
//! Как: по толчку (запуск, новый справочник монет, разблокировка/смена секретов, изменение настроек прогрева) для
//! каждой включённой биржи с venue.deposit_warm = да берутся направления справочника, где биржа — B (по одному на
//! монету), и по очереди, с паузой venue.deposit_warm_gap_ms между запросами, вызывается ТОТ ЖЕ метод шлюза
//! deposit_address с теми же монетой и сетью, что у предпроверки (precheck_deposit.rs::target) — адаптер кладёт адрес
//! в свой кэш, и предпроверка сделки берёт его из памяти. Новых запросов к биржам нет; адреса на диск не пишутся
//! (данные площадки — только из её API, в памяти процесса — как и раньше).
//! Что запоминается и когда проход прекращается — deposit_warm_plan.rs. Журнал core.deposit_warm — по бирже за проход:
//! сколько запрошено, получено, отказов, первая причина; самих адресов в журнале нет.
//! Кто вызывает: core/engine.rs (run — при запуске; kick — Core::secrets_changed), core/rebuild.rs (kick — новый
//! справочник монет).
//! От чего зависит: engine.rs, precheck_deposit.rs (target), deposit_warm_plan.rs, registry_sync.rs (enabled), шлюз.
//! Настройки: venue.deposit_warm, venue.deposit_warm_gap_ms (по площадке; docs/SETTINGS.md).

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::future::join_all;
use serde_json::json;
use tokio::sync::Notify;

use crate::core::types::{Direction, VenueId};
use crate::gateway::contract::VenueKind;
use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deposit_warm_plan::{After, Memo, Tally, pick};
use super::engine::{Core, rd};

/// Толчок прохода прогрева (разрешение копится — толчки во время прохода сливаются в один следующий).
static KICK: LazyLock<Notify> = LazyLock::new(Notify::new);
/// Перед следующим проходом забыть отказы по монетам (секреты или настройки прогрева изменились).
static RETRY_FAILED: AtomicBool = AtomicBool::new(false);

const ON: &str = "venue.deposit_warm";
const GAP: &str = "venue.deposit_warm_gap_ms";

/// Что делает: толкает проход прогрева. Принимает: retry_failed — заново запросить монеты, которые отказали.
/// Ошибок нет.
pub fn kick(retry_failed: bool) {
    if retry_failed {
        RETRY_FAILED.store(true, Ordering::SeqCst);
    }
    KICK.notify_one();
}

/// Память прогрева биржи: ответы по монетам и последняя записанная в журнал причина «не задано».
#[derive(Default)]
struct VenueState {
    memo: Memo,
    reported: Option<String>,
}

/// Задача прогрева: первый проход — сразу, дальше — по толчку. Работает, пока жива система. Ошибок не возвращает.
pub async fn run(core: Core) {
    // Изменение настроек прогрева (включили, задали паузу) — толчок с повтором отказов.
    let mut w = core.inner.deps.watch(&[ON]);
    tokio::spawn(async move {
        while w.next().await.is_some() {
            kick(true);
        }
    });
    let mut states: HashMap<VenueId, VenueState> = HashMap::new();
    kick(false);
    loop {
        KICK.notified().await;
        if RETRY_FAILED.swap(false, Ordering::SeqCst) {
            states.values_mut().for_each(|s| s.memo.forget_failed());
        }
        let venues: Vec<VenueId> = rd(&core.inner.kinds)
            .iter()
            .filter(|(v, k)| *k == VenueKind::Cex && on(&core, v))
            .map(|(v, _)| v.clone())
            .collect();
        if venues.is_empty() {
            continue;
        }
        // Снимок справочника (Arc, без блокировки на время прохода); направления только торгуемые (registry.rs).
        let book = core.coins();
        let dirs: Vec<Direction> = book.iter().flat_map(|c| c.directions()).collect();
        let jobs = venues.into_iter().map(|v| {
            let s = states.remove(&v).unwrap_or_default();
            let list = pick(dirs.iter().cloned(), &v);
            let core = core.clone();
            async move {
                let s = venue_pass(&core, &v, list, s).await;
                (v, s)
            }
        });
        for (v, s) in join_all(jobs).await {
            states.insert(v, s);
        }
    }
}

/// Включён ли прогрев у биржи: площадка включена (venue.enabled) и venue.deposit_warm = да. Не задано — нет.
fn on(core: &Core, v: &VenueId) -> bool {
    let at = At {
        venue: Some(&v.0),
        coin: None,
    };
    super::registry_sync::enabled(core, v) && core.inner.deps.settings.flag(ON, at).unwrap_or(false)
}

/// Что делает: проход прогрева одной биржи — по очереди, с паузой venue.deposit_warm_gap_ms между запросами.
/// Принимает: ядро, биржу, её направления (по одному на монету), её память. Возвращает: обновлённую память.
/// Ошибок нет: пауза не задана — один раз журнал core.not_configured и прохода нет; итог — журнал core.deposit_warm.
async fn venue_pass(
    core: &Core,
    v: &VenueId,
    list: Vec<Direction>,
    mut s: VenueState,
) -> VenueState {
    let deps = &core.inner.deps;
    let at = At {
        venue: Some(&v.0),
        coin: None,
    };
    let gap = match deps.settings.integer(GAP, at) {
        Ok(ms) => Duration::from_millis(ms),
        Err(e) => {
            if s.reported.as_deref() != Some(e.message.as_str()) {
                let msg = format!("Прогрев адресов депозита {} не идёт: {}", v.0, e.message);
                deps.log(Level::Warn, "core.not_configured", Some(&v.0), msg, None)
                    .await;
                s.reported = Some(e.message);
            }
            return s;
        }
    };
    s.reported = None;
    let gw = match deps.gw.get(v).await {
        Ok(g) => g,
        Err(_) => return s,
    };
    let mut t = Tally::default();
    for d in list {
        // Сети BSC у монеты нет — предпроверка откажет сама, запроса к бирже нет.
        let Ok(tg) = super::precheck_deposit::target(core, &d) else {
            continue;
        };
        let key = (tg.on.ticker.clone(), tg.net.clone());
        if !s.memo.todo(&key) {
            continue;
        }
        if t.asked > 0 {
            tokio::time::sleep(gap).await;
        }
        // Выключили посреди прохода — стоп.
        if !on(core, v) {
            t.stopped = Some("прогрев выключен".into());
            break;
        }
        t.asked += 1;
        let res = gw.deposit_address(&tg.on, &tg.net).await;
        let after = match &res {
            Ok(_) => {
                t.got += 1;
                s.memo.record(key, Ok(()))
            }
            Err(e) => {
                t.failed += 1;
                if t.first_err.is_none() {
                    t.first_err = Some(format!("{}: {}", d.asset.ticker, e.message));
                }
                s.memo.record(key, Err(&e.code))
            }
        };
        if after == After::Stop {
            if let Err(e) = &res {
                t.stopped = Some(format!("{:?}", e.code));
                t.locked = e.code == ErrorCode::SecretsLocked;
            }
            break;
        }
    }
    report(core, v, &t, &s.memo).await;
    s
}

/// Что делает: журнал core.deposit_warm за проход (кроме прохода без запросов и упора в закрытые секреты — Tally::silent).
/// Без адресов. Ошибок нет.
async fn report(core: &Core, v: &VenueId, t: &Tally, memo: &Memo) {
    if t.silent() {
        return;
    }
    let (ready, refused) = memo.counts();
    let level = if t.failed > 0 {
        Level::Warn
    } else {
        Level::Info
    };
    let msg = t.text(&v.0, ready, refused);
    let data = json!({
        "asked": t.asked, "got": t.got, "failed": t.failed,
        "ready_total": ready, "refused_total": refused,
        "first_error": t.first_err, "stopped": t.stopped,
    });
    core.inner
        .deps
        .log(level, "core.deposit_warm", Some(&v.0), msg, Some(data))
        .await;
}
