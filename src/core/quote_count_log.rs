//! Файл: core/quote_count_log.rs
//! Что делает: решение 348 — сводка «кто вызвал котировку» в журнал. Раз в log.quote_count_s секунд — одна запись
//! dex.quote_count (Info): сколько котировок DEX за интервал по каждой метке (signal, size, final, precheck, uncapped,
//! shadow, window, card, exec, check, unmarked — gateway/quote_purpose.rs), полных и быстрого пути, и в секунду. Счётчики
//! ведёт адаптер при входе в котировку (атомики, без записи на каждую котировку); здесь — только разница между
//! снимками. Не задана или 0 — сводки нет: задача ждёт смены настройки (без опроса по времени).
//! Решения 349, 352: в той же записи — сколько событий промежуточных пулов будили монеты и сколько монет (mid_wake.rs),
//! сколько котировок DEX сигнала взято из памяти по отпечатку и сколько заново (signal_dex.rs).
//! Решения 372–374: в той же записи (поле sieve) — сколько кандидатов отсечено границей (по USDT / по %), пропущено в
//! расчёт, «метод не действует» по классам причин, и сколько циклов DEX ↔ DEX отсечено (sieve_log.rs, gateway/rate_bound.rs);
//! решение 385 — контроль отсева циклов: cycle_audit, cycle_audit_violation, cycle_exempt.
//! Кто вызывает: core/engine.rs (задача при запуске ядра).
//! От чего зависит: gateway/quote_purpose.rs (snapshot), mid_wake.rs, signal_dex.rs (counts), deps.rs (журнал),
//! settings (подписка на изменения).
//! Настройки: log.quote_count_s.

use std::time::{Duration, Instant};

use serde_json::{Map, json};
use tokio::sync::broadcast::error::RecvError;

use crate::gateway::quote_purpose::{self, Who};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::Core;

/// Имя настройки интервала сводки.
const NAME: &str = "log.quote_count_s";

/// Задача сводки. Работает, пока жив канал настроек. Ошибок нет.
pub async fn run(core: Core) {
    // Подписка — до чтения: изменение между чтением и ожиданием не теряется.
    let mut rx = core.inner.deps.settings.subscribe();
    let mut prev = quote_purpose::snapshot();
    let mut prev_x = extra();
    let mut prev_s = sieve();
    let mut since = Instant::now();
    loop {
        let every = interval(&core);
        let Some(every) = every else {
            // Сводка выключена — ждать смены настройки; счёт с момента включения.
            match rx.recv().await {
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            }
            prev = quote_purpose::snapshot();
            prev_x = extra();
            prev_s = sieve();
            since = Instant::now();
            continue;
        };
        tokio::select! {
            // Ревью t160 Н7: срок — от начала интервала (since), смена настройки его не сдвигает.
            _ = tokio::time::sleep_until((since + every).into()) => {
                let now = quote_purpose::snapshot();
                let now_x = extra();
                let now_s = sieve();
                write(&core, &prev, &now, (prev_x, now_x), (&prev_s, &now_s), since.elapsed()).await;
                prev = now;
                prev_x = now_x;
                prev_s = now_s;
                since = Instant::now();
            }
            r = rx.recv() => {
                // Сменилась любая настройка — перечитать интервал (счёт продолжается).
                if let Err(RecvError::Closed) = r {
                    return;
                }
            }
        }
    }
}

/// Интервал сводки (log.quote_count_s > 0). None — не задана или 0. Ошибок нет.
fn interval(core: &Core) -> Option<Duration> {
    core.inner
        .deps
        .settings
        .integer(NAME, At::default())
        .ok()
        .filter(|s| *s > 0)
        .map(Duration::from_secs)
}

/// Одна запись dex.quote_count: разница счётчиков за интервал. Принимает: ядро, прошлый и новый снимок, длительность.
/// Ошибок нет.
async fn write(
    core: &Core,
    prev: &[(Who, u64, u64)],
    now: &[(Who, u64, u64)],
    x: ([u64; 4], [u64; 4]),
    sv: (&Counts, &Counts),
    took: Duration,
) {
    let secs = took.as_secs_f64().max(f64::MIN_POSITIVE);
    let mut data = Map::new();
    let mut parts = Vec::new();
    let mut total = 0u64;
    for ((w, full, fast), (_, full0, fast0)) in now.iter().zip(prev) {
        let (f, q) = (full.saturating_sub(*full0), fast.saturating_sub(*fast0));
        total += f + q;
        data.insert(
            w.name().into(),
            json!({ "full": f, "fast": q, "per_s": (f + q) as f64 / secs }),
        );
        if f + q > 0 {
            parts.push(format!("{} {}", w.name(), f + q));
        }
    }
    data.insert("total".into(), json!(total));
    // Решения 349, 352: пробуждения монет промежуточными пулами и котировки DEX сигнала из памяти за интервал.
    let names = [
        "mid_wake_events",
        "mid_wake_coins",
        "dex_memo_hits",
        "dex_memo_misses",
    ];
    let mut more = Vec::new();
    for (i, n) in names.iter().enumerate() {
        let v = x.1[i].saturating_sub(x.0[i]);
        data.insert((*n).into(), json!(v));
        if v > 0 {
            more.push(format!("{n} {v}"));
        }
    }
    // Решения 372–374: отсев кандидатов по границе (sieve_log.rs) и циклов DEX ↔ DEX (gateway/rate_bound.rs) за интервал.
    let mut sieve_data = Map::new();
    for (k, v) in sv.1 {
        let d = v.saturating_sub(sv.0.get(k).copied().unwrap_or(0));
        sieve_data.insert(k.clone(), json!(d));
        if d > 0 {
            more.push(format!("sieve {k} {d}"));
        }
    }
    data.insert("sieve".into(), serde_json::Value::Object(sieve_data));
    data.insert("seconds".into(), json!(secs));
    let per_s = total as f64 / secs;
    let mut msg = format!(
        "котировки DEX за {secs:.0} с: {total} ({per_s:.1}/с) — {}",
        if parts.is_empty() {
            "нет".to_string()
        } else {
            parts.join(", ")
        }
    );
    if !more.is_empty() {
        msg.push_str("; ");
        msg.push_str(&more.join(", "));
    }
    let d = Some(serde_json::Value::Object(data));
    core.inner
        .deps
        .log(Level::Info, "dex.quote_count", None, msg, d)
        .await;
}

/// Счётчики отсева с запуска по ключу.
type Counts = std::collections::BTreeMap<String, u64>;

/// Решения 372–374: счётчики отсева кандидатов (cut_usd, cut_pct, pass, na_<класс>) и циклов (cycle_pass, cycle_cut,
/// cycle_na) с запуска; решение 385 — контроль отсева циклов (cycle_audit — взято в полный счёт из cycle_cut,
/// cycle_audit_violation — записанных нарушений, cycle_exempt — циклов на путях, исключённых контролем). Ошибок нет.
fn sieve() -> Counts {
    let mut m = super::sieve_log::snapshot();
    let c = crate::gateway::rate_bound::cycle_counts();
    let names = [
        "cycle_pass",
        "cycle_cut",
        "cycle_na",
        "cycle_audit",
        "cycle_audit_violation",
        "cycle_exempt",
    ];
    for (k, v) in names.iter().zip(c) {
        m.insert((*k).to_string(), v);
    }
    m
}

/// Решения 349, 352: счётчики с запуска — [событий промежуточных пулов, разбуженных монет, котировок DEX сигнала из
/// памяти, заново]. Ошибок нет.
fn extra() -> [u64; 4] {
    let (events, coins) = super::mid_wake::counts();
    let (hits, misses) = super::signal_dex::counts();
    [events, coins, hits, misses]
}
