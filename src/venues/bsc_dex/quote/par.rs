//! Файл: venues/bsc_dex/quote/par.rs
//! Что делает: решение 336 (задача t157) — параллельный проход по независимым элементам внутри одного счёта адаптера
//! DEX (оценка путей в отборе лучших и в шаге сплита, оценка пулов при сборке графа, расчёт циклов DEX ↔ DEX) на
//! нескольких потоках: постоянный набор помощников quote/par_pool.rs (ревью t157 Н2 — без создания потока на каждый
//! вызов; только средства std, без новых зависимостей); у помощника — среда tokio вызывающего (Handle::enter).
//! Как устроено: элементы раздаются по одному из общего счётчика (поток, освободившийся раньше, берёт следующий —
//! нагрузка ровная); каждый итог кладётся с номером элемента, после — по номерам: порядок итогов ТОТ ЖЕ, что у прохода
//! по очереди, поэтому выбор по итогам (лучший путь, доля сплита, порядок рёбер) не меняется. Свой расчёт у каждого
//! элемента не зависит от других (только чтение общих данных).
//! Потоков — не больше want (настройка вызывающего: dex.quote_threads, dex.graph_threads) − 1 помощник + сам поток
//! счёта. Помощник берёт место общего предела cpu.threads_max (gateway/cpu_budget.rs) БЕЗ ожидания, ярусом счёта
//! (cpu_local.rs): мест нет — помощников меньше (вплоть до прохода по очереди). Без предела — want − 1 помощник.
//! Отмена: `alive` спрашивает только поток счёта (на каждом своём элементе; у счёта котировки — признак брошенного
//! расчёта quote/abandon.rs); ответ «нет» — помощники тоже останавливаются на ближайшем элементе, итог None.
//! Паника помощника — паника потока счёта (как у прохода по очереди).
//! want ≤ 1 или элементов меньше двух — проход по очереди в этом же потоке (ровно как до 336).
//! Кто вызывает: quote/route_best.rs, quote/graph_split.rs, quote/cycle_scan.rs, pools/graph_full.rs,
//! pools/graph_eval.rs.
//! От чего зависит: gateway/cpu_budget.rs, quote/cpu_local.rs, quote/par_pool.rs, ctx.rs, std, tokio (Handle).
//! Настройки: читает переданную вызывающим (want); cpu.threads_max, cpu.reserve_deal, cpu.reserve_calc — через
//! gateway/cpu_budget.rs; cpu.helper_threads — через par_pool.rs.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::gateway::cpu_budget;

use super::super::ctx::Ctx;
use super::{cpu_local, par_pool};

/// Сколько потоков просит настройка `name` (адаптер DEX). Не задана, 0 или 1 — 1. Ошибок нет.
pub fn want(ctx: &Ctx, name: &str) -> usize {
    ctx.int(name)
        .ok()
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 1)
        .unwrap_or(1)
}

/// То же, что map_init, без своего состояния у потока. Принимает и возвращает: как map_init.
pub fn map<I, R, F>(
    ctx: &Ctx,
    want: usize,
    items: &[I],
    f: F,
    alive: &dyn Fn() -> bool,
) -> Option<Vec<R>>
where
    I: Sync,
    R: Send,
    F: Fn(&I) -> R + Sync,
{
    map_init(ctx, want, items, || (), |_: &mut (), x: &I| f(x), alive)
}

/// Параллельный проход (см. шапку). Принимает: окружение (предел мест), сколько потоков (want), элементы, начальное
/// состояние потока (init — у каждого потока своё, например подготовка глубины с памятью внутри), расчёт элемента,
/// «работа жива» (спрашивает только поток счёта). Возвращает: итоги в порядке элементов; None — работа отменена.
/// Ошибок нет (ошибки — в итогах элементов).
pub fn map_init<I, R, S, N, F>(
    ctx: &Ctx,
    want: usize,
    items: &[I],
    init: N,
    f: F,
    alive: &dyn Fn() -> bool,
) -> Option<Vec<R>>
where
    I: Sync,
    R: Send,
    N: Fn() -> S + Sync,
    F: Fn(&mut S, &I) -> R + Sync,
{
    let n = want.min(items.len());
    // Места помощников: без ожидания; держатся до конца прохода. Помощники — из постоянного набора (par_pool.rs).
    let mut permits = Vec::new();
    let helpers = if n <= 1 {
        0
    } else {
        let cap = (n - 1).min(par_pool::ensure(ctx, n - 1));
        match cpu_budget::limits(&ctx.settings) {
            None => cap,
            Some(lim) => {
                let tier = cpu_local::tier();
                while permits.len() < cap {
                    match cpu_budget::try_acquire(lim, tier) {
                        Some(p) => permits.push(p),
                        None => break,
                    }
                }
                permits.len()
            }
        }
    };
    if helpers == 0 {
        let mut s = init();
        let mut out = Vec::with_capacity(items.len());
        for x in items {
            if !alive() {
                return None;
            }
            out.push(f(&mut s, x));
        }
        return Some(out);
    }
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    // Раздача по одному элементу; check — «жива ли работа» (у помощника всегда да, решает поток счёта).
    let work = |own: &mut S, check: &dyn Fn() -> bool| -> Vec<(usize, R)> {
        let mut got = Vec::new();
        loop {
            if stop.load(Ordering::Acquire) {
                break;
            }
            if !check() {
                stop.store(true, Ordering::Release);
                break;
            }
            let i = next.fetch_add(1, Ordering::Relaxed);
            let Some(x) = items.get(i) else { break };
            got.push((i, f(own, x)));
        }
        got
    };
    let (init, work) = (&init, &work);
    let sink: Mutex<Vec<(usize, R)>> = Mutex::new(Vec::new());
    let sink_ref = &sink;
    // Среда tokio вызывающего — и у помощника: записи журнала изнутри расчёта (например смена глубины пула,
    // pools/depth_log.rs через Handle::try_current) не теряются.
    let rt = tokio::runtime::Handle::try_current().ok();
    let rt = &rt;
    let scope = par_pool::Scope::new();
    for _ in 0..helpers {
        scope.spawn(move || {
            let _rt = rt.as_ref().map(|h| h.enter());
            let mut s = init();
            let got = work(&mut s, &|| true);
            sink_ref
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .extend(got);
        });
    }
    let mut s = init();
    let mut all = work(&mut s, alive);
    // Ждём начатые задачи помощников (не начатые не выполняются — их элементы досчитал этот поток).
    scope.finish();
    all.extend(sink.into_inner().unwrap_or_else(|p| p.into_inner()));
    drop(permits);
    if stop.load(Ordering::Acquire) {
        return None;
    }
    all.sort_unstable_by_key(|(i, _)| *i);
    Some(all.into_iter().map(|(_, r)| r).collect())
}
