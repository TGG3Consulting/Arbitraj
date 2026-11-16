//! Файл: core/quote_count_log.rs
//! Что делает: решение 348 — сводка «кто вызвал котировку» в журнал. Раз в log.quote_count_s секунд — одна запись
//! dex.quote_count (Info): сколько котировок DEX за интервал по каждой метке (signal, size, final, precheck, uncapped,
//! shadow, window, card, exec, check, unmarked — gateway/quote_purpose.rs), полных и быстрого пути, и в секунду. Счётчики
//! ведёт адаптер при входе в котировку (атомики, без записи на каждую котировку); здесь — только разница между
//! снимками. Не задана или 0 — сводки нет: задача ждёт смены настройки (без опроса по времени).
//! Кто вызывает: core/engine.rs (задача при запуске ядра).
//! От чего зависит: gateway/quote_purpose.rs (snapshot), deps.rs (журнал), settings (подписка на изменения).
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
            since = Instant::now();
            continue;
        };
        tokio::select! {
            _ = tokio::time::sleep(every) => {
                let now = quote_purpose::snapshot();
                write(&core, &prev, &now, since.elapsed()).await;
                prev = now;
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
async fn write(core: &Core, prev: &[(Who, u64, u64)], now: &[(Who, u64, u64)], took: Duration) {
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
    data.insert("seconds".into(), json!(secs));
    let per_s = total as f64 / secs;
    let msg = format!(
        "котировки DEX за {secs:.0} с: {total} ({per_s:.1}/с) — {}",
        if parts.is_empty() {
            "нет".to_string()
        } else {
            parts.join(", ")
        }
    );
    let d = Some(serde_json::Value::Object(data));
    core.inner
        .deps
        .log(Level::Info, "dex.quote_count", None, msg, d)
        .await;
}
