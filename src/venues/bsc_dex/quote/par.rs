//! Файл: venues/bsc_dex/quote/par.rs
//! Что делает: решение 336 (задача t157) — параллельный проход по независимым элементам внутри одного счёта адаптера
//! DEX (оценка путей в отборе лучших и в шаге сплита, оценка пулов при сборке графа, расчёт циклов DEX ↔ DEX) на
//! нескольких потоках (std::thread::scope — только средства std, без новых зависимостей).
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
//! От чего зависит: gateway/cpu_budget.rs, quote/cpu_local.rs, ctx.rs, std.
//! Настройки: читает переданную вызывающим (want); cpu.threads_max, cpu.reserve_deal, cpu.reserve_calc — через
//! gateway/cpu_budget.rs.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::gateway::cpu_budget;

use super::super::ctx::Ctx;
use super::cpu_local;

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
    // Места помощников: без ожидания; держатся до конца прохода.
    let mut permits = Vec::new();
    let helpers = if n <= 1 {
        0
    } else {
        match cpu_budget::limits(&ctx.settings) {
            None => n - 1,
            Some(lim) => {
                let tier = cpu_local::tier();
                while permits.len() < n - 1 {
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
    let mut all = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..helpers)
            .map(|_| {
                sc.spawn(move || {
                    let mut s = init();
                    work(&mut s, &|| true)
                })
            })
            .collect();
        let mut s = init();
        let mut all = work(&mut s, alive);
        for h in hs {
            match h.join() {
                Ok(v) => all.extend(v),
                Err(p) => std::panic::resume_unwind(p),
            }
        }
        all
    });
    drop(permits);
    if stop.load(Ordering::Acquire) {
        return None;
    }
    all.sort_unstable_by_key(|(i, _)| *i);
    Some(all.into_iter().map(|(_, r)| r).collect())
}
