//! Файл: gateway/step_meter.rs
//! Что делает: решение 265 — журнал HTTP-запросов к площадкам на задачу tokio: ядро ставит его на шаг сделки (ордер
//! покупки, пересчёт предпроверки), общий HTTP-обмен (gateway/http.rs exchange_full) на каждый запрос записывает
//! «метод путь», когда он начался от начала журнала, сколько шёл и чем кончился. Так в хронологии сделки видно каждый
//! запрос к бирже внутри ордера (пробный ордер, сам ордер, каждый опрос статуса, запрос сделок и комиссии) — без
//! изменения контракта шлюза (contract.rs) и без правок адаптеров. Путь — без строки запроса (в ней подпись и ключи).
//! Что не попадает (вне общего exchange_full): узел BSC и KyberSwap (gateway/http.rs exchange — их считает
//! calc_meter.rs), Bybit (свой обмен venues/bybit/http.rs), запросы задач, запущенных адаптером через spawn (task_local
//! не наследуется) — их время входит в шаг ядра целиком.
//! Кто вызывает: core/executor.rs, core/precheck.rs (scope, take), gateway/http.rs (active, record).
//! От чего зависит: tokio (task_local).
//! Настройки: нет — замер всегда, пока журнал стоит на задаче (одна короткая блокировка на запрос).

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Один HTTP-запрос: «метод путь», начало от начала журнала (мс), длительность (мс), итог («код 200» / причина сбоя).
#[derive(Debug, Clone, PartialEq)]
pub struct Req {
    pub name: String,
    pub at_ms: u128,
    pub ms: u128,
    pub outcome: String,
}

/// Журнал запросов шага (общий для задачи, Arc).
#[derive(Debug)]
pub struct Rec {
    started: Instant,
    list: Mutex<Vec<Req>>,
}

impl Rec {
    /// Новый журнал; отсчёт — с этого момента. Ошибок нет.
    pub fn new() -> Arc<Rec> {
        Arc::new(Rec {
            started: Instant::now(),
            list: Mutex::new(Vec::new()),
        })
    }

    /// Записанные запросы по порядку. Ошибок нет.
    pub fn take(&self) -> Vec<Req> {
        self.list.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

tokio::task_local! {
    static REC: Arc<Rec>;
}

/// Выполняет работу с журналом запросов. Принимает: журнал, работу. Возвращает: её результат. Ошибок нет.
pub async fn scope<F: Future>(rec: Arc<Rec>, work: F) -> F::Output {
    REC.scope(rec, work).await
}

/// Стоит ли журнал на текущей задаче (без него HTTP-обмен имя запроса не собирает). Ошибок нет.
pub fn active() -> bool {
    REC.try_with(|_| ()).is_ok()
}

/// Запрос кончился: имя, момент начала, итог. Журнала на задаче нет — ничего. Ошибок нет.
pub fn record(name: String, from: Instant, outcome: String) {
    let _ = REC.try_with(|r| {
        let req = Req {
            name,
            at_ms: from.saturating_duration_since(r.started).as_millis(),
            ms: from.elapsed().as_millis(),
            outcome,
        };
        r.list.lock().unwrap_or_else(|p| p.into_inner()).push(req);
    });
}
