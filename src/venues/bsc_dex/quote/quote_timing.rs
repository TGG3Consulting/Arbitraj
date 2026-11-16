//! Файл: venues/bsc_dex/quote/quote_timing.rs
//! Что делает: замер этапов одной котировки DEX (решение 312, задача t143) — куда уходит время котировки на пути
//! quote/mod.rs → cpu.rs → routing.rs (классика, граф). Включаемый: log.quote_timing = да И задан
//! log.quote_timing_min_ms — запись журнала dex.quote_timing (quote_timing_log.rs) на котировку, у которой ожидание до
//! старта работы + работа не меньше порога (0 — каждая). Не задана любая из двух или log.quote_timing = нет — замера нет:
//! одно чтение настройки log.quote_timing на котировку (start), обёртка timed с None — сразу работа; в точках этапов —
//! чтение признака потока (bool) без часов, выделений памяти и блокировок.
//! Как устроено: работа котировки идёт в одном потоке блокирующих задач (cpu.rs: spawn_blocking + block_on) — запись
//! этапов копится в локальной для потока структуре (thread_local, без блокировок, без await) и пишется в журнал один раз
//! в конце (timed → quote_timing_log.rs, отдельной задачей tokio — котировку не задерживает). Логика и порядок расчёта не
//! меняются: только часы (Instant) вокруг этапов. Вне timed (тесты, поиск циклов, другие вызовы) признака нет — ничего
//! не меряется.
//! Кто вызывает: quote/mod.rs (start, timed); quote/routing.rs, quote/graph_route.rs, quote/route_cache.rs (now, add);
//! quote/route_best.rs, quote/graph_split.rs (count).
//! От чего зависит: ctx.rs (настройки), quote/quote_timing_stages.rs (этапы, счётчики), quote/quote_timing_log.rs
//! (запись), core/types.rs (Quote, Side).
//! Настройки: log.quote_timing, log.quote_timing_min_ms (вся система).

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::time::{Duration, Instant};

use rust_decimal::Decimal;

use crate::core::types::{Quote, Side};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::quote_timing_log;
use super::route_paths::RoutePath;

pub use super::quote_timing_stages::{COUNTS, Count, STAGES, Stage, TOP};

/// Начало замера одной котировки: момент перед cpu::run и порог записи.
#[derive(Clone, Copy)]
pub struct Start {
    pub t0: Instant,
    pub min_ms: u64,
}

/// Что котируется (для записи).
pub struct Meta {
    pub ticker: String,
    pub side: Side,
    pub qty: Decimal,
}

/// Накопленный замер котировки (в потоке счёта).
pub struct Rec {
    pub start: Start,
    /// Начало работы в потоке счёта.
    pub work: Instant,
    pub ns: [u64; STAGES],
    pub calls: [u32; STAGES],
    pub counts: [u64; COUNTS],
}

thread_local! {
    /// Признак «котировка этого потока меряется» — быстрая проверка в точках этапов.
    static ON: Cell<bool> = const { Cell::new(false) };
    static REC: RefCell<Option<Rec>> = const { RefCell::new(None) };
}

/// Замер включён? log.quote_timing = да и задан log.quote_timing_min_ms — начало замера (часы — сейчас). Иначе None
/// (одно чтение настройки). Ошибок нет.
pub fn start(ctx: &Ctx) -> Option<Start> {
    if !ctx.flag("log.quote_timing").unwrap_or(false) {
        return None;
    }
    let min_ms = ctx.int("log.quote_timing_min_ms").ok()?;
    Some(Start {
        t0: Instant::now(),
        min_ms,
    })
}

/// Снимает признак потока при выходе из timed (и при панике работы).
struct Clear;

impl Drop for Clear {
    fn drop(&mut self) {
        ON.with(|c| c.set(false));
        REC.with(|r| r.borrow_mut().take());
    }
}

/// Работа котировки под замером. on = None — только работа (ничего не меряется). Some — запись этапов в потоке, где
/// идёт работа (поток счёта cpu.rs), и запись журнала в конце (quote_timing_log::report). Принимает: (окружение, начало,
/// что котируется), работу. Возвращает: итог работы как есть. Ошибок своих нет.
pub async fn timed<F>(on: Option<(Ctx, Start, Meta)>, work: F) -> GatewayResult<Quote>
where
    F: Future<Output = GatewayResult<Quote>>,
{
    let held = on.map(|(ctx, start, meta)| {
        let rec = Rec {
            start,
            work: Instant::now(),
            ns: [0; STAGES],
            calls: [0; STAGES],
            counts: [0; COUNTS],
        };
        REC.with(|r| *r.borrow_mut() = Some(rec));
        ON.with(|c| c.set(true));
        (ctx, meta, Clear)
    });
    let out = work.await;
    if let Some((ctx, meta, clear)) = held {
        let rec = REC.with(|r| r.borrow_mut().take());
        drop(clear);
        if let Some(rec) = rec {
            quote_timing_log::report(&ctx, &meta, &rec, &out);
        }
    }
    out
}

/// Котировка этого потока меряется? Ошибок нет.
pub fn active() -> bool {
    ON.with(Cell::get)
}

/// Часы начала этапа, если котировка меряется. Ошибок нет.
pub fn now() -> Option<Instant> {
    active().then(Instant::now)
}

/// Часы начала этапа по уже прочитанному признаку (в цикле — без чтения признака потока). Ошибок нет.
pub fn now_if(on: bool) -> Option<Instant> {
    on.then(Instant::now)
}

/// Добавляет к этапу время от `t` до сейчас (t = None — ничего). Ошибок нет.
pub fn add(stage: Stage, t: Option<Instant>) {
    let Some(t) = t else {
        return;
    };
    let ns = u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX);
    REC.with(|r| {
        if let Some(rec) = r.borrow_mut().as_mut() {
            let i = stage as usize;
            rec.ns[i] = rec.ns[i].saturating_add(ns);
            rec.calls[i] = rec.calls[i].saturating_add(1);
        }
    });
}

/// Этап под часами: выполняет `f` и, если `on`, добавляет её время к этапу. Логика `f` не меняется. Принимает: этап,
/// признак замера, работу этапа. Возвращает: итог работы как есть. Ошибок своих нет.
pub fn stage<T>(s: Stage, on: bool, f: impl FnOnce() -> T) -> T {
    let t = now_if(on);
    let out = f();
    add(s, t);
    out
}

/// Счётчики путей на допуск хопов: путей и хопов (только если `on`). Ошибок нет.
pub fn paths_in(on: bool, paths: &[RoutePath]) {
    if !on {
        return;
    }
    count(Count::PathsIn, paths.len() as u64);
    count(Count::Hops, paths.iter().map(|p| p.hops() as u64).sum());
}

/// Добавляет к счётчику `n`, если котировка меряется. Ошибок нет.
pub fn count(c: Count, n: u64) {
    if !active() {
        return;
    }
    REC.with(|r| {
        if let Some(rec) = r.borrow_mut().as_mut() {
            let i = c as usize;
            rec.counts[i] = rec.counts[i].saturating_add(n);
        }
    });
}

/// Длительность в мс (дробная). Ошибок нет.
pub fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
