//! Файл: venues/bsc_dex/quote/quote_timing.rs
//! Что делает: замер этапов работы одной котировки DEX в потоке счёта (решение 312, задача t143) — куда уходит время
//! на пути quote/mod.rs → cpu.rs → routing.rs (классика, граф). Включает замер quote/quote_timing_outer.rs (Outer::begin:
//! log.quote_timing = да И задан log.quote_timing_min_ms); он же меряет начало и хвост Quoting::priced и пишет запись.
//! Здесь — только работа: обёртка timed с None (замера нет) сразу ведёт работу; в точках этапов — чтение признака
//! потока (bool) без часов, выделений памяти и блокировок.
//! Как устроено: работа котировки идёт в одном потоке блокирующих задач (cpu.rs: spawn_blocking + block_on) — запись
//! этапов копится в локальной для потока структуре (thread_local, без блокировок, без await) и в конце работы один раз
//! передаётся замеру priced (старт — quote_timing_outer::start; конец — quote_timing_outer::hand — запись журнала после хвоста priced; priced уже брошен — запись
//! сразу). Логика и порядок расчёта не меняются: только часы (Instant) вокруг этапов. Вне timed (тесты, поиск циклов,
//! другие вызовы) признака нет — ничего не меряется.
//! Решение 336: Clock — часы этапа до конца области; части этапа «классика» (direct_route.rs, classic_direct.rs,
//! usable_reuse.rs).
//! Кто вызывает: quote/mod.rs (timed — через Outer::run); quote/routing.rs (now, add); quote/graph_route.rs (active,
//! now_if, add, stage, paths_in, count); quote/route_cache.rs (now, add); quote/route_best.rs, quote/graph_split.rs
//! (count); quote/quote_timing_outer.rs, quote/quote_timing_log.rs (Rec, Meta, ms); тест tests/bsc_dex_quote_timing_312.rs.
//! От чего зависит: quote/quote_timing_stages.rs (этапы, счётчики), quote/quote_timing_outer.rs (Run, start, hand),
//! quote/abandon.rs (брошен ли счёт), quote/route_paths.rs (RoutePath), core/types.rs (Quote, Side).
//! Настройки: нет (log.quote_timing, log.quote_timing_min_ms читает quote_timing_outer.rs).

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::time::{Duration, Instant};

use rust_decimal::Decimal;

use crate::core::types::{Quote, Side};
use crate::gateway::errors::GatewayResult;

use super::abandon;
use super::quote_timing_outer::{self, Outcome, Run};
use super::route_paths::RoutePath;

pub use super::quote_timing_stages::{COUNTS, Count, STAGES, Stage, TOP};

/// Что котируется (для записи).
pub struct Meta {
    pub ticker: String,
    pub side: Side,
    pub qty: Decimal,
}

/// Накопленный замер котировки (в потоке счёта).
pub struct Rec {
    /// Вход в cpu::run (ожидание до старта работы — от него до `work`).
    pub cpu_in: Instant,
    /// Начало работы в потоке счёта.
    pub work: Instant,
    /// Конец работы в потоке счёта.
    pub end: Instant,
    /// Счёт брошен по сроку расчёта (quote/abandon.rs) — на конец работы.
    pub abandoned: bool,
    pub ns: [u64; STAGES],
    pub calls: [u32; STAGES],
    pub counts: [u64; COUNTS],
}

thread_local! {
    /// Признак «котировка этого потока меряется» — быстрая проверка в точках этапов.
    static ON: Cell<bool> = const { Cell::new(false) };
    static REC: RefCell<Option<Rec>> = const { RefCell::new(None) };
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
/// идёт работа (поток счёта cpu.rs), и в конце — передача замера priced (quote_timing_outer::hand; запись журнала —
/// там). Принимает: связь с замером priced (Outer::run), работу. Возвращает: итог работы как есть. Ошибок своих нет.
pub async fn timed<F>(on: Option<Run>, work: F) -> GatewayResult<Quote>
where
    F: Future<Output = GatewayResult<Quote>>,
{
    // Старт работы — под замком связи (quote_timing_outer::start): priced уже записан (брошен до старта) — без замера.
    let held = on.filter(quote_timing_outer::start).map(|run| {
        let now = Instant::now();
        let rec = Rec {
            cpu_in: run.cpu_in,
            work: now,
            end: now,
            abandoned: false,
            ns: [0; STAGES],
            calls: [0; STAGES],
            counts: [0; COUNTS],
        };
        REC.with(|r| *r.borrow_mut() = Some(rec));
        ON.with(|c| c.set(true));
        (run, Clear)
    });
    let out = work.await;
    if let Some((run, clear)) = held {
        let end = Instant::now();
        let rec = REC.with(|r| r.borrow_mut().take());
        drop(clear);
        if let Some(mut rec) = rec {
            rec.end = end;
            rec.abandoned = abandon::is_set();
            quote_timing_outer::hand(run, rec, Outcome::of(&out));
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

/// Решение 336: часы этапа до конца области видимости (Drop) — для работы с ранним выходом `?` внутри (замыкание
/// сборки котировки классики, direct_route.rs). Без замера — часов нет.
pub struct Clock(Stage, Option<Instant>);

impl Clock {
    /// Часы этапа `s` с этой минуты (если котировка меряется). Ошибок нет.
    pub fn new(s: Stage) -> Clock {
        Clock(s, now())
    }
}

impl Drop for Clock {
    fn drop(&mut self) {
        add(self.0, self.1.take());
    }
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
