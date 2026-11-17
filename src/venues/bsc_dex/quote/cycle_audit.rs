//! Файл: venues/bsc_dex/quote/cycle_audit.rs
//! Что делает: решение 385 — контроль отсева циклов DEX ↔ DEX на живом боте (страховка, НЕ доказательство:
//! доказательство — офлайн-тесты на записанных данных, tests/bsc_dex_bound_cycles_t171.rs). Тот же смысл, что у
//! контроля кандидатов (core/sieve_audit.rs, решения 375, 377, 378), та же настройка sieve.audit_share: такая доля ВСЕХ
//! отсечённых циклов (ρ < 1, quote/bound_cycle.rs) идёт в ОБЫЧНЫЙ полный счёт цикла (quote/cycle_scan.rs signal:
//! cycle_opt::best, форма исполнения, сигнал) — сигнал отдаётся ядру как любой найденный цикл (core/cycle_watch.rs берёт
//! исполнимый с чистой прибылью > 0; сделка одна — сигнал один, тот же, что был бы без отсева).
//! Отбор без случайности (Audit, один на проход поиска: доля читается и множество путей проверяется один раз): n-й
//! отсечённый прохода (n — порядковый, атомик — потоки dex.graph_threads) с начальным сдвигом, равным номеру прохода,
//! идёт в контроль, если ⌊(сдвиг + n + 1)·доля⌋ > ⌊(сдвиг + n)·доля⌋ (hit). Сдвиг на 1 каждый проход: при неизменном
//! порядке циклов выбранные позиции смещаются, за 1/доля проходов контроль проходит каждую позицию (ревью t171, Н3).
//! Нарушение: у контрольного сигнала валовая > 0 или чистая > 0 (чистая > 0 — условие ядра; валовая > 0 — то, что
//! граница исключает напрямую: ρ < 1 ⇒ выход < вход), а ρ < 1 посчитан на ТЕХ ЖЕ ногах (Leg несёт копию записи пула —
//! тот же снимок пулов). Найденное нарушение сразу встаёт в очередь процесса (verify) — не теряется, даже если поиск
//! брошен или отменён; очередь разбирает flush (venue.rs dex_cycles после каждого поиска, удачного или нет): запись
//! Error core.sieve_violation (вид «цикл»: маршрут, ρ, вход, выход, прибыль), уведомление Telegram тем же кодом (Notice →
//! ядро → notify), счётчик cycle_audit_violation и ТОЛЬКО ПОСЛЕ записи — путь в множество исключённых: отсев к нему
//! больше не применяется до перезапуска (счётчик cycle_exempt), прибыльный цикл не теряется при доле < 1. Разбор — по
//! одному (замок разбора): одна запись на путь при одновременных поисках; запись удаляется из очереди только после
//! записи в журнал (брошенный разбор — запись остаётся и уйдёт со следующим).
//! Кто вызывает: quote/cycle_scan.rs (Audit: start, gate, verify), venue.rs (flush), тесты (hit, violated, report, Audit,
//! flush).
//! От чего зависит: quote/bound_cycle.rs (cycle_sieve), quote/cycle.rs (CycleSignal), quote/route_calc.rs (Leg), send/stats.rs (Notice), gateway/rate_bound.rs
//! (счётчики), ctx.rs (журнал), settings.
//! Настройки: sieve.audit_share (вся система; общая с контролем кандидатов).

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use alloy_primitives::Address;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::PoolRef;
use crate::gateway::events::MarketEvent;
use crate::gateway::rate_bound::cycle_count;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::send::stats::Stats;
use super::bound_cycle::CycleSieve;
use super::cycle::CycleSignal;
use super::route_calc::Leg;

/// Код записи журнала и события уведомлений (notify/catalog.rs) — тот же, что у контроля кандидатов.
pub const CODE: &str = "core.sieve_violation";

/// Путь цикла: пулы и токен входа каждого шага по порядку.
pub type Key = Vec<(PoolRef, Address)>;

/// Номер прохода поиска — сдвиг отбора.
static PASSES: AtomicU64 = AtomicU64::new(0);
/// Пути с записанным нарушением (до перезапуска): отсев к ним не применяется.
static WRONG: LazyLock<Mutex<HashSet<Key>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
/// Множество WRONG не пусто (чтобы не брать замок на каждый цикл).
static ANY_WRONG: AtomicBool = AtomicBool::new(false);
/// Найденные, ещё не записанные нарушения.
static QUEUE: LazyLock<Mutex<VecDeque<Violation>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));
/// Замок разбора очереди (одна запись на путь).
static FLUSH: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Нарушение доказательства отсева цикла: путь, текст и данные записи.
#[derive(Debug, Clone)]
pub struct Violation {
    pub key: Key,
    pub text: String,
    pub data: Value,
}

/// Контроль одного прохода поиска циклов (см. шапку). Общий для потоков прохода (только атомик и чтение).
pub struct Audit {
    /// Доля 0 < доля ≤ 1; None — контроля нет.
    share: Option<Decimal>,
    /// Сдвиг отбора — номер прохода.
    shift: u64,
    /// Сколько отсечённых этого прохода уже рассмотрено.
    seen: AtomicU64,
    /// Есть ли пути с записанным нарушением (снимок на начало прохода).
    any_wrong: bool,
}

/// Итог отсева цикла с контролем (Audit::gate).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gate {
    /// Отсечён и не выбран контролем — полного счёта нет.
    Cut,
    /// Полный счёт; Some(ρ) — контрольный (отсечён, выбран долей): сверить сигнал с ρ (verify).
    Calc(Option<Decimal>),
}

/// Что делает: идёт ли в контроль отсечённый с порядковым номером n (с учётом сдвига) при доле: ⌊(n+1)·доля⌋ >
/// ⌊n·доля⌋. Чистая. Принимает: номер, долю. Возвращает: true — контроль. Ошибок нет (доля ≤ 0 — false, > 1 — как 1).
pub fn hit(n: u64, share: Decimal) -> bool {
    let s = share.min(Decimal::ONE);
    if s <= Decimal::ZERO {
        return false;
    }
    let a = (Decimal::from(n) * s).floor();
    let b = (Decimal::from(n.saturating_add(1)) * s).floor();
    b > a
}

impl Audit {
    /// Что делает: контроль на проход: доля sieve.audit_share и снимок «есть исключённые пути» — один раз; сдвиг — номер
    /// прохода. Принимает: окружение. Возвращает: Audit. Ошибок нет (доля не задана или ≤ 0 — контроля нет).
    pub fn start(ctx: &Ctx) -> Audit {
        let share = ctx
            .settings
            .decimal("sieve.audit_share", At::default())
            .ok()
            .filter(|s| *s > Decimal::ZERO);
        Audit {
            share,
            shift: PASSES.fetch_add(1, Ordering::Relaxed),
            seen: AtomicU64::new(0),
            any_wrong: ANY_WRONG.load(Ordering::Relaxed),
        }
    }

    /// Что делает: отсев цикла с контролем: путь с записанным нарушением не отсекается (счётчик cycle_exempt); иначе
    /// граница (quote/bound_cycle.rs cycle_sieve); отсечённый — Cut или, если выбран долей (pick), Calc(Some(ρ)).
    /// Принимает: окружение, ноги. Возвращает: Gate. Ошибок нет.
    pub fn gate(&self, ctx: &Ctx, legs: &[Leg]) -> Gate {
        if self.exempt(legs) {
            cycle_count(5);
            return Gate::Calc(None);
        }
        match super::bound_cycle::cycle_sieve(ctx, legs) {
            CycleSieve::Cut(rho) if self.pick() => Gate::Calc(Some(rho)),
            CycleSieve::Cut(_) => Gate::Cut,
            CycleSieve::Pass | CycleSieve::Na => Gate::Calc(None),
        }
    }

    /// Что делает: путь уже с записанным нарушением — не отсекать (путь считается только если такие есть).
    /// Принимает: ноги. Возвращает: true — не отсекать. Ошибок нет.
    pub fn exempt(&self, legs: &[Leg]) -> bool {
        self.any_wrong
            && WRONG
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&key(legs))
    }

    /// Что делает: идёт ли этот отсечённый в контрольный полный счёт (hit по номеру со сдвигом); да — счётчик
    /// cycle_audit. Принимает: ничего. Возвращает: true — контроль. Ошибок нет.
    pub fn pick(&self) -> bool {
        let Some(share) = self.share else {
            return false;
        };
        let n = self.seen.fetch_add(1, Ordering::Relaxed);
        let h = hit(self.shift.wrapping_add(n), share);
        if h {
            cycle_count(3);
        }
        h
    }

    /// Что делает: сверка контрольного счёта: нарушение — сразу в очередь записи (см. шапку). Принимает: ρ тех же ног,
    /// сигнал полного счёта, ноги. Ошибок нет.
    pub fn verify(&self, rho: Decimal, s: &CycleSignal, legs: &[Leg]) {
        if violated(rho, s) {
            let v = report(rho, s, key(legs));
            QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push_back(v);
        }
    }
}

/// Что делает: путь цикла по ногам (пул и токен входа каждого шага). Принимает: ноги. Возвращает: ключ. Ошибок нет.
pub fn key(legs: &[Leg]) -> Key {
    legs.iter()
        .map(|l| (l.entry.pool.reference.clone(), l.token_in))
        .collect()
}

/// Что делает: нарушает ли полный счёт доказательство отсева: ρ < 1 и прибыль > 0 (валовая или чистая). Чистая.
/// Принимает: ρ цикла (на тех же ногах), сигнал полного счёта. Возвращает: true — нарушение. Ошибок нет.
pub fn violated(rho: Decimal, s: &CycleSignal) -> bool {
    rho < Decimal::ONE
        && (s.gross_profit > Decimal::ZERO || s.net_profit.is_some_and(|n| n > Decimal::ZERO))
}

/// Что делает: разбор очереди нарушений (см. шапку): запись, Telegram, счётчик, затем путь — в исключённые. Принимает:
/// окружение (журнал, площадка), канал событий ядру. Ошибок нет (сбой записи журнала выводит сам журнал).
pub async fn flush(ctx: &Ctx, stats: &Stats) {
    let empty = QUEUE.lock().unwrap_or_else(|e| e.into_inner()).is_empty();
    if empty {
        return;
    }
    let _one = FLUSH.lock().await;
    loop {
        let next = QUEUE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .front()
            .cloned();
        let Some(v) = next else { break };
        let known = WRONG
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&v.key);
        if !known {
            ctx.log(Level::Error, CODE, v.text.clone(), Some(v.data))
                .await;
            stats.emit(MarketEvent::Notice {
                venue: ctx.venue(),
                code: CODE.to_string(),
                text: v.text,
            });
            cycle_count(4);
            WRONG
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(v.key);
            ANY_WRONG.store(true, Ordering::Relaxed);
        }
        QUEUE.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
    }
}

/// Что делает: текст и данные записи нарушения. Чистая. Принимает: ρ, сигнал, путь. Возвращает: Violation. Ошибок нет.
pub fn report(rho: Decimal, s: &CycleSignal, key: Key) -> Violation {
    let route: Vec<String> = s
        .hops
        .iter()
        .map(|h| format!("{} {:?} {:?}", h.family, h.kind, h.pool))
        .collect();
    let net = s
        .net_profit
        .map_or_else(|| "не посчитана".to_string(), |n| n.round_dp(8).to_string());
    let text = format!(
        "Цикл DEX ↔ DEX {b}: НАРУШЕНИЕ ДОКАЗАТЕЛЬСТВА ОТСЕВА — граница ρ = {rho} < 1 исключает прибыль, а полный счёт на \
         тех же данных пулов (блок {blk}) дал вход {i} {b}, выход {o} {b}, валовая {g} {b}, чистая {net} {b}; маршрут: {r}. \
         Отсев к этому пути отключён до перезапуска; цикл идёт обычным путём",
        b = s.base_ticker,
        blk = s.block,
        i = s.input.normalize(),
        o = s.output.normalize(),
        g = s.gross_profit.round_dp(8),
        r = route.join(" → "),
    );
    let data = json!({ "kind": "cycle", "rho": rho, "signal": s });
    Violation { key, text, data }
}
