//! Файл: core/uncapped_queue.rs
//! Что делает: фоновая очередь расчёта «без потолка» (решение 141). Боевой путь её не ждёт: candidates.rs и
//! executor.rs только кладут работу (память под короткой блокировкой) и идут дальше. Работа считается ВНЕ рабочих
//! потоков tokio — в потоке пула блокирующих задач (spawn_blocking + Handle::block_on, как счёт котировки DEX решения
//! 131, venues/bsc_dex/quote/cpu.rs), классом срочности Background (у очереди узла BSC — последний).
//! Правила: uncapped.enabled = нет / не задана — ничего не ставится; одновременно — не больше uncapped.max_concurrent;
//! кандидат по направлению — не чаще раза в uncapped.min_interval_ms (от начала прошлого расчёта), повтор в это время
//! или пока идёт расчёт сливается — ждёт только последний; сделка (id) — без интервала; ждущих (ожидающие и ждущие
//! места) больше uncapped.queue_max — пропуск со счётчиком «не посчитано: очередь полна». Решение 197 (Тигран, 06.11.2026:
//! «это окно наш измеритель упущенной прибыли»; живой бот: очередь всегда полна, у 24 из 25 сделок записи нет): сделка
//! не выбрасывается и не входит в uncapped.queue_max (свой счётчик deals_waiting), а свободное место расчёта сначала
//! получает ждущая сделка — кандидат, получив место, пока ждут сделки, отдаёт его и ждёт (turn). Итог — uncapped_store.rs
//! (SQLite) и строка «Кандидатов» (поле uncapped). Не задана настройка — сбой со счётчиком и причиной на экране.
//! Решение 217: работа сделки несёт снимок рынка момента решения (core/frozen.rs) — расчёт идёт по нему; сбой расчёта
//! сделки запоминается по её id (uncapped_store::deal_failed) для сообщения о закрытии.
//! Кто вызывает: core/uncapped_entry.rs (submit).
//! От чего зависит: uncapped_permit.rs (место расчёта), uncapped.rs (compute), uncapped_state.rs, uncapped_store.rs, gateway/urgency.rs, settings.
//! Настройки: uncapped.enabled, uncapped.max_concurrent, uncapped.min_interval_ms, uncapped.queue_max.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::core::types::Direction;
use crate::gateway::urgency::{self, Urgency};
use crate::settings::read::At;

use super::engine::{Core, lk};
use super::frozen::Frozen;
use super::uncapped_state::{Job, Key};
use super::uncapped_types::Battle;

/// Целая настройка. Ошибки: текст — не задана.
fn int(core: &Core, name: &str) -> Result<u64, String> {
    core.inner
        .deps
        .settings
        .integer(name, At::default())
        .map_err(|e| e.message)
}

/// Постановка (правила — в шапке). Принимает: ядро, ключ, направление, бой, источник котировок (решение 217: None —
/// живые, у сделки — снимок момента решения или причина, почему его нет). Ошибок нет: сбой настройки — в счётчик и причину.
pub fn submit(
    core: &Core,
    key: Key,
    d: &Direction,
    battle: Battle,
    src: Option<Result<Frozen, String>>,
) {
    if !super::uncapped_entry::enabled(core) {
        return;
    }
    let st = &core.inner.uncapped;
    let cfg = int(core, "uncapped.min_interval_ms")
        .and_then(|i| Ok((i, int(core, "uncapped.queue_max")?)));
    let (interval, queue_max) = match cfg {
        Ok(c) => c,
        Err(e) => return super::uncapped_store::failed(core, e),
    };
    let interval = Duration::from_millis(interval);
    let job = Job {
        dir: d.clone(),
        battle,
        src,
    };
    let deal = matches!(key, Key::Deal(_));
    // Решение 197: сделку очередь не выбрасывает.
    let full = || !deal && st.waiting.load(Ordering::Acquire) as u64 >= queue_max;
    let mut slots = lk(&st.slots);
    let slot = slots.entry(key.clone()).or_default();
    let wait = match (&key, slot.last) {
        (Key::Cand(_), Some(l)) => interval.saturating_sub(l.elapsed()),
        _ => Duration::ZERO,
    };
    if slot.running || !wait.is_zero() {
        if slot.pending.is_some() {
            slot.pending = Some(job);
            lk(&st.stats).merged += 1;
            return;
        }
        if full() {
            lk(&st.stats).skipped_full += 1;
            return;
        }
        slot.pending = Some(job);
        counter(st, deal).fetch_add(1, Ordering::AcqRel);
        if !slot.running {
            later(core, key, wait);
        }
        return;
    }
    if full() {
        lk(&st.stats).skipped_full += 1;
        return;
    }
    slot.running = true;
    slot.last = Some(Instant::now());
    counter(st, deal).fetch_add(1, Ordering::AcqRel);
    drop(slots);
    tokio::spawn(run(core.clone(), key, job));
}

/// Счётчик ждущих: сделки — deals_waiting (без предела), кандидаты — waiting (uncapped.queue_max). Ошибок нет.
fn counter(st: &super::uncapped_state::State, deal: bool) -> &AtomicUsize {
    if deal { &st.deals_waiting } else { &st.waiting }
}

/// Запуск ожидающей работы ключа через `wait`. Ошибок нет.
fn later(core: &Core, key: Key, wait: Duration) {
    let core = core.clone();
    tokio::spawn(async move {
        tokio::time::sleep(wait).await;
        let job = {
            let mut slots = lk(&core.inner.uncapped.slots);
            let Some(slot) = slots.get_mut(&key) else {
                return;
            };
            if slot.running {
                return;
            }
            let Some(job) = slot.pending.take() else {
                return;
            };
            slot.running = true;
            slot.last = Some(Instant::now());
            job
        };
        run(core, key, job).await;
    });
}

/// Расчёт работы в потоке блокирующих задач, итог — в хранилище; затем ожидающая работа ключа. Ошибок нет.
async fn run(core: Core, key: Key, job: Job) {
    let st = &core.inner.uncapped;
    let deal = matches!(key, Key::Deal(_));
    let held = super::uncapped_permit::permit(&core, deal).await;
    if counter(st, deal).fetch_sub(1, Ordering::AcqRel) == 1 && deal {
        // Ждущих сделок не осталось — кандидаты снова могут брать место.
        st.turn.notify_waiters();
    }
    match held {
        Ok(held) => {
            let rt = tokio::runtime::Handle::current();
            let c = core.clone();
            let (d, battle, src) = (job.dir.clone(), job.battle, job.src);
            let r = tokio::task::spawn_blocking(move || {
                let _held = held;
                rt.block_on(urgency::scope(
                    Urgency::Background,
                    super::uncapped::compute(&c, &d, battle, src),
                ))
            })
            .await
            .map_err(|e| format!("поток расчёта без потолка прерван: {e}"))
            .and_then(|r| r);
            match (r, &key) {
                (Ok(rec), _) => super::uncapped_store::save(&core, &key, rec).await,
                (Err(e), Key::Deal(id)) => super::uncapped_store::deal_failed(&core, id, e),
                (Err(e), Key::Cand(_)) => super::uncapped_store::failed(&core, e),
            }
        }
        Err(e) => match &key {
            Key::Deal(id) => super::uncapped_store::deal_failed(&core, id, e),
            Key::Cand(_) => super::uncapped_store::failed(&core, e),
        },
    }
    let mut slots = lk(&st.slots);
    let Some(slot) = slots.get_mut(&key) else {
        return;
    };
    slot.running = false;
    if slot.pending.is_some() {
        let wait = match (&key, slot.last) {
            (Key::Cand(_), Some(l)) => int(&core, "uncapped.min_interval_ms")
                .map(|i| Duration::from_millis(i).saturating_sub(l.elapsed()))
                .unwrap_or_default(),
            _ => Duration::ZERO,
        };
        drop(slots);
        later(&core, key, wait);
    } else if matches!(key, Key::Deal(_)) {
        slots.remove(&key);
    }
}
