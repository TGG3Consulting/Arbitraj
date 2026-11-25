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

/// Attach the venue's final order status to the request already recorded for this task.
pub(crate) fn annotate_response(request: &str, note: &str) {
    let _ = REC.try_with(|rec| {
        let mut requests = rec.list.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = requests.last_mut().filter(|entry| entry.name == request) {
            entry.outcome.push_str(", ");
            entry.outcome.push_str(note);
        }
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn poloniex_btw_order_details_carries_the_cancellation_into_the_deal_trace() {
        let rec = Rec::new();
        let request = "GET /orders/648304427677188096";
        scope(rec.clone(), async {
            record(request.to_string(), Instant::now(), "код 200".into());
            let (_, note) = crate::venues::poloniex::trade::cancel_note(
                "BTW_USDT",
                "648304427677188096",
                &serde_json::json!({"state": "CANCELED", "cancelReason": 1003}),
            )
            .unwrap();
            annotate_response("POST /orders", &note);
            assert_eq!(rec.take()[0].outcome, "код 200");
            annotate_response(request, &note);
        })
        .await;
        let rows = rec.take();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].outcome.starts_with("код 200, "));
        assert!(rows[0].outcome.contains("cancelReason 1003"));
    }
}
