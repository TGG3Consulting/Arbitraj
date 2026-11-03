//! Файл: gateway/calc_meter.rs
//! Что делает: счётчик внешних запросов расчёта кандидата (решение 129) — общий для ядра и адаптеров. Ядро ставит
//! счётчик на задачу расчёта (`scope`), адаптер на каждом запросе к узлу BSC или к агрегатору сообщает, сколько
//! вызовов ушло, сколько запрос ждал своей очереди (темп узла bsc.node_http_rps / темп kyber.rps) и сколько шёл сам
//! (`record`). Ядро по снимкам до и после этапа видит, сколько запросов и времени этап потратил на узел и агрегатор,
//! — так причина «расчёт не уложился в срок» называет этап и показывает, ждал ли он узел или считал сам.
//! Ядро о площадках не знает: вид источника — два значения здесь, в шлюзе. Счётчик задаётся на задачу tokio
//! (task_local, как класс срочности urgency.rs); новая задача его не наследует — запросы задач, запущенных адаптером
//! через spawn, не считаются (их время всё равно входит в этап, который их ждёт). Не задан — `record` ничего не делает.
//! Кто вызывает: core/candidates.rs (scope), core/calc_timing.rs (snapshot), venues/bsc_dex/node/pace.rs и
//! venues/kyberswap/rest.rs (record).
//! От чего зависит: tokio (task_local).
//! Настройки: нет (замер всегда — несколько атомарных сложений на запрос).

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Куда ушёл запрос.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Узел сети BSC (JSON-RPC по HTTP).
    Node,
    /// Внешний агрегатор (KyberSwap).
    Aggregator,
}

/// Счётчики одного источника: вызовов, ожидание очереди (мкс), сам запрос (мкс).
#[derive(Default, Debug)]
struct Counters {
    calls: AtomicU64,
    wait_us: AtomicU64,
    send_us: AtomicU64,
}

/// Счётчик расчёта. Общий для задачи расчёта (Arc), сложение — атомарное.
#[derive(Default, Debug)]
pub struct Meter {
    node: Counters,
    agg: Counters,
}

/// Снимок счётчиков одного источника.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snap {
    pub calls: u64,
    pub wait_us: u64,
    pub send_us: u64,
}

impl Snap {
    /// Разница «после − до» (этап). Ошибок нет.
    pub fn since(self, before: Snap) -> Snap {
        Snap {
            calls: self.calls.saturating_sub(before.calls),
            wait_us: self.wait_us.saturating_sub(before.wait_us),
            send_us: self.send_us.saturating_sub(before.send_us),
        }
    }
}

impl Meter {
    /// Снимок обоих источников: (узел, агрегатор). Ошибок нет.
    pub fn snapshot(&self) -> (Snap, Snap) {
        let s = |c: &Counters| Snap {
            calls: c.calls.load(Ordering::Relaxed),
            wait_us: c.wait_us.load(Ordering::Relaxed),
            send_us: c.send_us.load(Ordering::Relaxed),
        };
        (s(&self.node), s(&self.agg))
    }
}

tokio::task_local! {
    static METER: Arc<Meter>;
}

/// Выполняет работу со счётчиком. Принимает: счётчик, работу. Возвращает: её результат. Ошибок нет.
pub async fn scope<F: Future>(meter: Arc<Meter>, work: F) -> F::Output {
    METER.scope(meter, work).await
}

/// Запрос ушёл: источник, число вызовов в нём (пачка JSON-RPC — N), ожидание очереди и время самого запроса.
/// Счётчика на задаче нет — ничего. Ошибок нет.
pub fn record(source: Source, calls: usize, wait: Duration, send: Duration) {
    let _ = METER.try_with(|m| {
        let c = match source {
            Source::Node => &m.node,
            Source::Aggregator => &m.agg,
        };
        c.calls.fetch_add(calls as u64, Ordering::Relaxed);
        c.wait_us
            .fetch_add(wait.as_micros() as u64, Ordering::Relaxed);
        c.send_us
            .fetch_add(send.as_micros() as u64, Ordering::Relaxed);
    });
}
