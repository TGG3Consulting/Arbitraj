//! Файл: venues/bsc_dex/quote/cycle_audit.rs
//! Что делает: решение 385 — контроль отсева циклов DEX ↔ DEX на живом боте (страховка, НЕ доказательство:
//! доказательство — офлайн-тесты на записанных данных, tests/bsc_dex_bound_cycles_t171.rs). Тот же смысл, что у
//! контроля кандидатов (core/sieve_audit.rs, решения 375, 377, 378), та же настройка sieve.audit_share: такая доля ВСЕХ
//! отсечённых циклов (ρ < 1, quote/bound_cycle.rs) идёт в ОБЫЧНЫЙ полный счёт цикла (quote/cycle_scan.rs signal:
//! cycle_opt::best, форма исполнения, сигнал) — сигнал отдаётся ядру как любой найденный цикл (core/cycle_watch.rs берёт
//! исполнимый с чистой прибылью > 0; сделка одна — сигнал один, тот же, что был бы без отсева). Отбор без случайности:
//! накопитель доли (каждый отсечённый прибавляет долю, на ≥ 1 — контроль и минус 1), от показа не зависит.
//! Нарушение: у контрольного сигнала валовая прибыль > 0 или чистая > 0 (чистая > 0 — условие, по которому ядро берёт
//! цикл; валовая > 0 — то, что граница исключает напрямую: ρ < 1 ⇒ выход < вход), а ρ < 1 посчитан на ТЕХ ЖЕ ногах
//! (Leg несёт копию записи пула — тот же снимок пулов, без оговорки «данные могли смениться», как у кандидатов).
//! Нарушение — запись Error core.sieve_violation (вид «цикл»: маршрут, ρ, вход, выход, прибыль) и уведомление Telegram тем
//! же кодом (пишет venue.rs dex_cycles); путь цикла запоминается до перезапуска, и отсев к нему больше не применяется
//! (каждый следующий счёт — полный: найденный прибыльный цикл не теряется из-за доли < 1, повтора записи нет).
//! Кто вызывает: quote/cycle_scan.rs (pick, key, exempt, check), venue.rs (CODE), тесты (step, violated, report).
//! От чего зависит: quote/cycle.rs (CycleSignal), quote/route_calc.rs (Leg), gateway/rate_bound.rs (счётчики), settings.
//! Настройки: sieve.audit_share (вся система; общая с контролем кандидатов).

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use alloy_primitives::Address;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::PoolRef;
use crate::gateway::rate_bound::cycle_count;
use crate::settings::read::At;

use super::super::ctx::Ctx;
use super::cycle::CycleSignal;
use super::route_calc::Leg;

/// Код записи журнала и события уведомлений (notify/catalog.rs) — тот же, что у контроля кандидатов.
pub const CODE: &str = "core.sieve_violation";

/// Путь цикла: пулы и токен входа каждого шага по порядку.
pub type Key = Vec<(PoolRef, Address)>;

/// Накопитель доли.
static ACC: LazyLock<Mutex<Decimal>> = LazyLock::new(|| Mutex::new(Decimal::ZERO));

/// Пути, на которых контроль нашёл нарушение (до перезапуска): отсев к ним не применяется.
static WRONG: LazyLock<Mutex<HashSet<Key>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Нарушение доказательства отсева цикла: текст и данные записи.
#[derive(Debug, Clone)]
pub struct Violation {
    pub text: String,
    pub data: Value,
}

/// Что делает: шаг накопителя доли. Принимает: накопитель, долю (None, ≤ 0 — контроля нет; > 1 — как 1).
/// Возвращает: true — этот отсечённый идёт в контроль (накопитель дошёл до 1, минус 1). Ошибок нет.
pub fn step(acc: &mut Decimal, share: Option<Decimal>) -> bool {
    let Some(share) = share
        .filter(|s| *s > Decimal::ZERO)
        .map(|s| s.min(Decimal::ONE))
    else {
        return false;
    };
    *acc += share;
    if *acc < Decimal::ONE {
        return false;
    }
    *acc -= Decimal::ONE;
    true
}

/// Что делает: идёт ли этот отсечённый цикл в контрольный полный счёт (sieve.audit_share, накопитель); да — счётчик
/// cycle_audit. Принимает: окружение. Возвращает: true — контроль. Ошибок нет.
pub fn pick(ctx: &Ctx) -> bool {
    let share = ctx
        .settings
        .decimal("sieve.audit_share", At::default())
        .ok();
    let hit = step(&mut ACC.lock().unwrap_or_else(|e| e.into_inner()), share);
    if hit {
        cycle_count(3);
    }
    hit
}

/// Что делает: путь цикла по ногам (пул и токен входа каждого шага). Принимает: ноги. Возвращает: ключ. Ошибок нет.
pub fn key(legs: &[Leg]) -> Key {
    legs.iter()
        .map(|l| (l.entry.pool.reference.clone(), l.token_in))
        .collect()
}

/// Что делает: на этом пути контроль уже находил нарушение — отсев к нему не применяется. Принимает: ключ пути.
/// Возвращает: true — не отсекать. Ошибок нет.
pub fn exempt(k: &Key) -> bool {
    WRONG.lock().unwrap_or_else(|e| e.into_inner()).contains(k)
}

/// Что делает: нарушает ли полный счёт доказательство отсева: ρ < 1 и прибыль > 0 (валовая или чистая). Чистая.
/// Принимает: ρ цикла (на тех же ногах), сигнал полного счёта. Возвращает: true — нарушение. Ошибок нет.
pub fn violated(rho: Decimal, s: &CycleSignal) -> bool {
    rho < Decimal::ONE
        && (s.gross_profit > Decimal::ZERO || s.net_profit.is_some_and(|n| n > Decimal::ZERO))
}

/// Что делает: сверка контрольного счёта (см. шапку): нарушение — счётчик cycle_audit_violation, путь запоминается
/// (exempt), запись к отправке. Принимает: ρ, сигнал, ключ пути. Возвращает: нарушение или None. Ошибок нет.
pub fn check(rho: Decimal, s: &CycleSignal, k: Key) -> Option<Violation> {
    if !violated(rho, s) {
        return None;
    }
    cycle_count(4);
    WRONG.lock().unwrap_or_else(|e| e.into_inner()).insert(k);
    Some(report(rho, s))
}

/// Что делает: текст и данные записи нарушения. Чистая. Принимает: ρ, сигнал. Возвращает: Violation. Ошибок нет.
pub fn report(rho: Decimal, s: &CycleSignal) -> Violation {
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
    Violation { text, data }
}
