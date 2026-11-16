//! Файл: venues/bsc_dex/node/req_trail.rs
//! Что делает: решение 313 — список запросов к узлу BSC одной работы по отдельности: у каждого запроса (одно место
//! очереди темпа node/pace.rs на N вызовов) — сколько вызовов в нём, сколько он ждал своей очереди и сколько шёл сам.
//! В отличие от счётчика расчёта (gateway/calc_meter.rs — суммы по этапу) здесь каждый запрос отдельной строкой: замер
//! перечитывания пула по событию (pools/refresh_timing.rs) показывает «ожидание в очереди узла / сам запрос» по шагам
//! (slot0, liquidity, слова карты…). Список ставится на задачу tokio (task_local, как класс срочности
//! gateway/urgency.rs); новая задача его не наследует. Не задан — `record` ничего не делает (логика очереди не меняется:
//! pace.rs только сообщает уже измеренные им времена).
//! Кто вызывает: node/pace.rs (record), pools/refresh_event.rs (scope).
//! От чего зависит: tokio (task_local).
//! Настроек нет (включает вызывающий — log.pool_refresh_timing, решение 313).

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Один запрос к узлу: вызовов в нём (пачка JSON-RPC — N), ожидание очереди темпа, время самого запроса.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub calls: usize,
    pub wait: Duration,
    pub send: Duration,
}

/// Список запросов работы. Общий (Arc), запись — под коротким замком без await.
#[derive(Debug, Clone, Default)]
pub struct Trail(Arc<Mutex<Vec<Step>>>);

impl Trail {
    /// Забирает записанные запросы по порядку. Ошибок нет.
    pub fn take(&self) -> Vec<Step> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

tokio::task_local! {
    static TRAIL: Trail;
}

/// Выполняет работу со списком запросов. Принимает: список, работу. Возвращает: её результат. Ошибок нет.
pub async fn scope<F: Future>(trail: Trail, work: F) -> F::Output {
    TRAIL.scope(trail, work).await
}

/// Запрос ушёл: число вызовов, ожидание очереди, время запроса. Списка на задаче нет — ничего. Ошибок нет.
pub fn record(calls: usize, wait: Duration, send: Duration) {
    let _ = TRAIL.try_with(|t| {
        t.0.lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Step { calls, wait, send });
    });
}
