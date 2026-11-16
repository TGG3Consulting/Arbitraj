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
//! Решение 245, дополнение (Тигран 09.11): расчёт наблюдения окна максимального объёма (submit_watch, uncapped_watch.rs) —
//! ключ кандидата направления, те же интервал и предел очереди; ожидающий расчёт ключа он не вытесняет (расчёт кандидата
//! вытесняет его); итог — только окно (uncapped_watch::saved); после каждого расчёта ключа кандидата — uncapped_watch::next.
//! Кто вызывает: core/uncapped_entry.rs (submit), core/uncapped_watch.rs (submit_watch).
//! От чего зависит: uncapped_permit.rs (место расчёта), uncapped.rs (compute), uncapped_state.rs, uncapped_store.rs, gateway/urgency.rs, settings.
//! Настройки: uncapped.enabled, uncapped.max_concurrent, uncapped.min_interval_ms, uncapped.queue_max.

use std::future::Future;
use std::pin::Pin;
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
    put(core, key, d, battle, src, false)
}

/// Решение 245, дополнение (Тигран 09.11): расчёт наблюдения окна максимального объёма (uncapped_watch.rs) — ключ
/// кандидата направления, те же интервал, предел очереди и место расчёта; ожидающий расчёт ключа не вытесняет.
/// Принимает: ядро, направление, бой прошлого расчёта направления. Ошибок нет.
pub fn submit_watch(core: &Core, d: &Direction, battle: Battle) {
    put(core, Key::Cand(d.clone()), d, battle, None, true)
}

/// Постановка работы (submit, submit_watch). Ошибок нет.
fn put(
    core: &Core,
    key: Key,
    d: &Direction,
    battle: Battle,
    src: Option<Result<Frozen, String>>,
    watch: bool,
) {
    if !super::uncapped_entry::enabled(core) {
        return;
    }
    let st = &core.inner.uncapped;
    let cfg = int(core, "uncapped.min_interval_ms")
        .and_then(|i| Ok((i, int(core, "uncapped.queue_max")?)));
    let (interval, queue_max) = match cfg {
        Ok(c) => c,
        // Сбой настройки у сделки — причина по её id (сообщение о закрытии), у кандидата — общий счётчик.
        Err(e) => {
            return match &key {
                Key::Deal(id) => super::uncapped_store::deal_failed(core, id, e),
                Key::Cand(_) => super::uncapped_store::failed(core, e),
            };
        }
    };
    let interval = Duration::from_millis(interval);
    let job = Job {
        dir: d.clone(),
        battle,
        src,
        watch,
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
            // Решение 245, дополнение: расчёт наблюдения ожидающий расчёт ключа не вытесняет.
            if !watch {
                slot.pending = Some(job);
            }
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

/// Будущее, которое run ведёт в потоке расчёта (block_on): работа под классом срочности Background. Работа
/// принимается только из кучи (`Pin<Box<F>>`): размер итога не зависит от размера работы (решение 301, дополнение —
/// стек; сторож — tests/bsc_dex_quote_cpu_stack.rs). pub — точка входа для тестов-сторожей (боевой вызов — run).
/// Принимает: работу. Возвращает: будущее с её итогом. Ошибок нет.
pub fn scoped<F: Future>(work: Pin<Box<F>>) -> impl Future<Output = F::Output> {
    // Решение 348: назначение «без потолка» (замер «кто вызвал котировку»; признак — указатель-обёртка).
    urgency::scope(
        Urgency::Background,
        crate::gateway::quote_purpose::scope(
            crate::gateway::quote_purpose::Purpose::Uncapped,
            work,
        ),
    )
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
            let (d, battle, src) = (job.dir.clone(), job.battle.clone(), job.src);
            let r = tokio::task::spawn_blocking(move || {
                let _held = held;
                // Решение 301 (дополнение — стек): будущее расчёта — сразу в кучу (Box::pin), обёртка признака
                // (scoped, async fn — хранит и аргумент, и будущее, его содержащее) держит только указатель: стек
                // потока блокирующих задач (2 МиБ) не несёт расчёт под обёрткой по значению (как quote/cpu.rs run).
                // В кучу — здесь, а не до spawn_blocking: будущее заимствует c и d замыкания, а перенос до
                // spawn_blocking требует Send будущего расчёта — по коду не установлено. Одно выделение на расчёт.
                // Ревью t132 В3: в отладочной сборке будущее сначала создаётся в кадре замыкания и копируется в кучу;
                // место под него остаётся в кадре на всё время block_on — 11704 байт (замер run 43776529778:
                // uncapped::compute 11704, под urgency::scope по значению 23440, сейчас scoped 40) из 2 МиБ стека
                // потока — риск не реален, оставлено так (обоснование — цифра).
                let work = Box::pin(super::uncapped::compute(&c, &d, battle, src));
                rt.block_on(scoped(work))
            })
            .await
            .map_err(|e| format!("поток расчёта без потолка прерван: {e}"))
            .and_then(|r| r);
            match (r, &key) {
                // Решение 245, дополнение: расчёт наблюдения ведёт только окно максимального объёма.
                (Ok(rec), Key::Cand(d)) if job.watch => {
                    super::uncapped_watch::saved(&core, d, rec).await
                }
                (Ok(rec), _) => super::uncapped_store::save(&core, &key, rec).await,
                (Err(e), Key::Deal(id)) => super::uncapped_store::deal_failed(&core, id, e),
                (Err(e), Key::Cand(_)) => super::uncapped_store::failed(&core, e),
            }
            // Решение 245, дополнение: окно наблюдается — следующий расчёт наблюдения (с интервалом ключа).
            if let Key::Cand(d) = &key {
                super::uncapped_watch::next(&core, d, job.battle);
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
