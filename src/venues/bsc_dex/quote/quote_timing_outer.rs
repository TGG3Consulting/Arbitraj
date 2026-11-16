//! Файл: venues/bsc_dex/quote/quote_timing_outer.rs
//! Что делает: замер всей котировки DEX (решение 312, доработка t143) — Quoting::priced целиком (до работы: decimals,
//! цена газа и прочее; cpu::run: ожидание места и работа в потоке счёта, возврат итога; после работы: газ разрешений,
//! обёртки BNB, чаевые) и быстрого пути quote/fills.rs (решение 250). Включаемый: log.quote_timing = да И задан
//! log.quote_timing_min_ms — запись журнала dex.quote_timing (quote_timing_log.rs) на котировку, у которой всё время
//! (от входа в priced / быстрый путь до конца) не меньше порога (0 — каждая). Иначе Outer пустой: одно чтение
//! log.quote_timing на котировку, дальше — без часов (now = None, add/run/back — ничего; итог в finish — замыкание,
//! без замера не вызывается: Outcome не строится).
//! Как устроено: часы до и после работы — в future priced (Option<Instant> и Box; Send), этапы работы — в потоке счёта
//! (quote_timing.rs). Работа в конце отдаёт свой замер в общую связь (Link, std Mutex — только у меряемой котировки,
//! без await под замком). Запись — одна на котировку, пишет тот, кто закончил последним:
//! - priced дошёл до конца — finish (после хвоста, с итогом priced);
//! - priced брошен (срок расчёта ядра) после конца работы — Drop Outer (dropped = да, итог работы);
//! - priced брошен после старта работы в потоке счёта, до её конца — работа пишет сама в конце (hand; dropped = да);
//! - priced брошен в cpu::run до старта работы (ожидание места dex.quote_cpu_max или очередь spawn_blocking) — Drop
//!   Outer пишет сам (dropped = да, unstarted = да, без полей работы) и закрывает связь: старт работы (start, первый
//!   опрос timed — под тем же замком) видит закрытие и идёт без замера, hand не будет — ни потери, ни второй записи;
//! - priced брошен до cpu::run или ошибка до работы — запись без полей работы.
//!
//! Логика и порядок расчёта не меняются: только часы вокруг шагов.
//! Кто вызывает: quote/mod.rs (Quoting::priced: begin, now, add, run, back, finish), quote/fills.rs (быстрый путь:
//! begin_fast, now, add, finish), quote/quote_timing.rs (start — старт работы, hand — конец работы в потоке счёта); тест
//! tests/bsc_dex_quote_timing_312.rs.
//! От чего зависит: ctx.rs (настройки), quote/quote_timing.rs (Rec, Meta), quote/quote_timing_stages.rs (Part),
//! quote/quote_timing_log.rs (запись), gateway/errors.rs.
//! Настройки: log.quote_timing, log.quote_timing_min_ms (вся система).

use std::sync::{Arc, Mutex};
use std::time::Instant;

use rust_decimal::Decimal;

use crate::core::types::Side;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::quote_timing::{Meta, Rec};
use super::quote_timing_log;
use super::quote_timing_stages::{PARTS, Part};

/// Итог котировки для записи: ok, код ошибки, у быстрого пути — ответил ли он (true — полная котировка не нужна).
#[derive(Clone)]
pub struct Outcome {
    pub ok: bool,
    pub error: Option<String>,
    pub answered: Option<bool>,
}

impl Outcome {
    /// Итог по результату. Ошибок нет.
    pub fn of<T>(out: &GatewayResult<T>) -> Self {
        Outcome {
            ok: out.is_ok(),
            error: out.as_ref().err().map(|e| format!("{:?}", e.code)),
            answered: None,
        }
    }
}

/// Замер снаружи работы (в future priced / быстрого пути).
pub struct Body {
    pub ctx: Ctx,
    pub meta: Meta,
    pub min_ms: u64,
    /// Быстрый путь quote/fills.rs.
    pub fast: bool,
    /// Вход в priced / быстрый путь.
    pub t0: Instant,
    /// Вход в cpu::run и возврат из него (None — не было).
    pub cpu_in: Option<Instant>,
    pub cpu_out: Option<Instant>,
    /// Брошен в cpu::run до старта работы в потоке счёта (Drop; замера работы нет и не будет).
    pub unstarted: bool,
    /// Части (Part), нс.
    pub parts: [u64; PARTS],
    link: Link,
}

/// Общее у замера priced и работы: замер работы (с её итогом) или брошенный замер priced; started — работа начата в
/// потоке счёта (start), closed — priced уже записан (Drop): работа, не начатая к этому, идёт без замера.
#[derive(Default)]
pub struct Shared {
    work: Option<(Rec, Outcome)>,
    left: Option<Box<Body>>,
    started: bool,
    closed: bool,
}

/// Связь замера priced с работой в потоке счёта.
pub type Link = Arc<Mutex<Shared>>;

/// Что уходит в работу (quote_timing::timed): связь и вход в cpu::run.
pub struct Run {
    pub link: Link,
    pub cpu_in: Instant,
}

/// Замер котировки снаружи работы; пустой — замера нет.
pub struct Outer(Option<Box<Body>>);

impl Outer {
    /// Начало замера Quoting::priced: log.quote_timing = да и задан log.quote_timing_min_ms — часы сейчас; иначе пустой
    /// (одно чтение настройки). Принимает: окружение, тикер, сторону, объём. Ошибок нет.
    pub fn begin(ctx: &Ctx, ticker: &str, side: Side, qty: Decimal) -> Outer {
        Self::open(ctx, ticker, side, qty, false)
    }

    /// Начало замера быстрого пути quote/fills.rs (запись с fast = да). Как begin.
    pub fn begin_fast(ctx: &Ctx, ticker: &str, side: Side, qty: Decimal) -> Outer {
        Self::open(ctx, ticker, side, qty, true)
    }

    /// Общее начало begin / begin_fast. Принимает: окружение, тикер, сторону, объём, признак быстрого пути.
    /// Возвращает: замер или пустой. Ошибок нет.
    fn open(ctx: &Ctx, ticker: &str, side: Side, qty: Decimal, fast: bool) -> Outer {
        if !ctx.flag("log.quote_timing").unwrap_or(false) {
            return Outer(None);
        }
        let Ok(min_ms) = ctx.int("log.quote_timing_min_ms") else {
            return Outer(None);
        };
        let meta = Meta {
            ticker: ticker.to_string(),
            side,
            qty,
        };
        Outer(Some(Box::new(Body {
            ctx: ctx.clone(),
            meta,
            min_ms,
            fast,
            t0: Instant::now(),
            cpu_in: None,
            cpu_out: None,
            unstarted: false,
            parts: [0; PARTS],
            link: Link::default(),
        })))
    }

    /// Замер идёт? Ошибок нет.
    pub fn is_on(&self) -> bool {
        self.0.is_some()
    }

    /// Часы начала части, если замер идёт. Ошибок нет.
    pub fn now(&self) -> Option<Instant> {
        self.0.is_some().then(Instant::now)
    }

    /// Добавляет к части время от `t` до сейчас (t = None — ничего). Ошибок нет.
    pub fn add(&mut self, p: Part, t: Option<Instant>) {
        if let (Some(b), Some(t)) = (self.0.as_mut(), t) {
            let ns = u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX);
            let i = p as usize;
            b.parts[i] = b.parts[i].saturating_add(ns);
        }
    }

    /// Вход в cpu::run: отметка и связь для quote_timing::timed (None — замера нет). Ошибок нет.
    pub fn run(&mut self) -> Option<Run> {
        let b = self.0.as_mut()?;
        let now = Instant::now();
        b.cpu_in = Some(now);
        Some(Run {
            link: b.link.clone(),
            cpu_in: now,
        })
    }

    /// Возврат из cpu::run (итог любой). Ошибок нет.
    pub fn back(&mut self) {
        if let Some(b) = self.0.as_mut() {
            b.cpu_out = Some(Instant::now());
        }
    }

    /// Конец котировки: запись (если не меньше порога) с итогом `out()` и замером работы, если она была. Итог —
    /// замыкание: без замера не вызывается (Outcome не строится). Ошибок нет.
    pub fn finish(mut self, out: impl FnOnce() -> Outcome) {
        if let Some(b) = self.0.take() {
            let work = lock(&b.link).work.take();
            let out = out();
            quote_timing_log::report(&b, work.as_ref().map(|(r, _)| r), Some(&out), false);
        }
    }
}

impl Drop for Outer {
    /// priced / быстрый путь брошен до finish: работа кончилась, не начиналась или ещё не начата в потоке счёта —
    /// запись сейчас (dropped = да; не начата — unstarted = да, связь закрыта); работа идёт — замер priced отдаётся ей
    /// (hand пишет в конце работы).
    fn drop(&mut self) {
        let Some(mut b) = self.0.take() else {
            return;
        };
        // Решение «кто пишет» — под одним замком с hand (иначе запись может потеряться).
        let link = b.link.clone();
        let mut g = lock(&link);
        // В cpu::run, но работа ещё не начата в потоке счёта (ожидание места или очередь spawn_blocking): hand не будет.
        let unstarted = b.cpu_in.is_some() && b.cpu_out.is_none() && !g.started;
        // Работы не было, она уже отдана, уже не придёт (cpu::run вернулся) или не начата — пишем сейчас; связь
        // закрыта — работа, не начатая к этому, идёт без замера (start).
        if b.cpu_in.is_none() || b.cpu_out.is_some() || g.work.is_some() || unstarted {
            g.closed = true;
            b.unstarted = unstarted;
            let work = g.work.take();
            drop(g);
            let (rec, out) = (work.as_ref().map(|(r, _)| r), work.as_ref().map(|(_, o)| o));
            quote_timing_log::report(&b, rec, out, true);
            return;
        }
        // Своя связь замеру в ожидании не нужна — без кольца ссылок (Arc → Shared → Body → Arc).
        b.link = Link::default();
        g.left = Some(b);
    }
}

/// Старт работы в потоке счёта (первый опрос quote_timing::timed): под замком связи — priced ещё не записан (Drop) —
/// работа начата (замер; Drop отдаст замер priced ей, hand запишет); записан — false (работа без замера, hand не
/// будет). Принимает: связь. Возвращает: мерить ли работу. Ошибок нет.
pub fn start(run: &Run) -> bool {
    let mut g = lock(&run.link);
    if g.closed {
        return false;
    }
    g.started = true;
    true
}

/// Конец работы в потоке счёта (quote_timing::timed): priced уже брошен — запись сразу (dropped = да); иначе замер
/// работы ждёт priced (finish / Drop). Принимает: связь, замер работы, итог работы. Ошибок нет.
pub fn hand(run: Run, rec: Rec, out: Outcome) {
    let mut g = lock(&run.link);
    match g.left.take() {
        Some(b) => {
            drop(g);
            quote_timing_log::report(&b, Some(&rec), Some(&out), true);
        }
        None => g.work = Some((rec, out)),
    }
}

/// Замок связи (отравленный — как есть: в нём только замеры). Ошибок нет.
fn lock(link: &Link) -> std::sync::MutexGuard<'_, Shared> {
    link.lock().unwrap_or_else(|p| p.into_inner())
}
