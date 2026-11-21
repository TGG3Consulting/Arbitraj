//! Файл: core/patience_scope.rs
//! Что делает: решение 201 — срок «терпения» ручной работы и продажи сделки (gateway/patience.rs): пока площадка
//! выжидает паузу после HTTP 429, запрос ждёт её конец, а не отказывает («Продать сейчас» RWA на Bitget 06.11 —
//! «пауза после HTTP 429 — ещё 828 мс»). Не задана exec.rate_limit_wait_max_ms — как раньше (отказ сразу).
//! Кто вызывает: core/commands.rs, manual.rs, manual_halted.rs, manual_usdt.rs (вокруг risk_count::manual), sell.rs.
//! От чего зависит: gateway/patience.rs, settings.
//! Настройки: exec.rate_limit_wait_max_ms (глобальная, необязательная).

use std::future::Future;
use std::time::Duration;

use super::engine::Core;

/// Срок терпения. None — не задан или ноль (как раньше). Ошибок нет.
pub fn max(core: &Core) -> Option<Duration> {
    core.inner
        .deps
        .settings
        .integer("exec.rate_limit_wait_max_ms", Default::default())
        .ok()
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
}

/// Выполняет работу с терпением (если срок задан). Принимает: ядро, работу. Возвращает: её итог. Ошибок нет.
pub async fn with<F: Future>(core: &Core, work: F) -> F::Output {
    match max(core) {
        Some(d) => crate::gateway::patience::scope(d, work).await,
        None => work.await,
    }
}
