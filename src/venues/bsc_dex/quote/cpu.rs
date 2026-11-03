//! Файл: venues/bsc_dex/quote/cpu.rs
//! Что делает: счёт маршрута котировки DEX вне рабочих потоков tokio (решение 129, часть 2 — голодание расчётов при
//! включённой MEXC). Счёт (граф маршрутов, перебор путей, сплит, прямые пулы и пары через промежуточную монету) —
//! чистый CPU без отдачи управления: в рабочем потоке он держал рантайм (таймеры, чтение WebSocket, другие расчёты), а
//! срок расчёта profit.calc_timeout_ms срабатывал, только когда счёт сам отдаст управление. Здесь работа идёт в потоке
//! пула блокирующих задач (spawn_blocking) и ведётся там же (`Handle::block_on`) — редкие запросы к узлу внутри счёта
//! (decimals нового токена, запись в журнал) ждут в этом потоке, а не в рабочем. Вызывающий ждёт итог как обычный
//! await: срок расчёта (tokio::time::timeout ядра) срабатывает вовремя, брошенный счёт доживает один свой шаг в своём
//! потоке и выбрасывается. Класс срочности (urgency), счётчик запросов расчёта (calc_meter) и признак финальной
//! котировки (quote_stage) — признаки задачи tokio — переносятся в поток счёта.
//! Предел одновременных счётов — dex.quote_cpu_max (необязательная; 0 или не задана — без предела): сверх него счёт
//! ждёт места (ожидание — внутри срока расчёта и видно в замере этапов как время котировки без запросов к узлу).
//! Кто вызывает: quote/mod.rs (Quoting::quote — выбор маршрута).
//! От чего зависит: ctx.rs, gateway/{urgency, calc_meter, quote_stage}.rs, tokio.
//! Настройки: dex.quote_cpu_max [bsc_dex].

use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::{calc_meter, quote_stage, urgency};

use super::super::ctx::Ctx;

/// Предел одновременных счётов: (значение настройки, семафор). Один на процесс (адаптер DEX один).
static LIMIT: Mutex<Option<(u64, Arc<Semaphore>)>> = Mutex::new(None);

/// Место под счёт по dex.quote_cpu_max. None — предела нет (не задана или 0). Смена значения на лету — новый
/// семафор: идущие счёты доживают на старом (кратко возможно старый предел + новый). Ошибок нет.
async fn permit(ctx: &Ctx) -> Option<OwnedSemaphorePermit> {
    let n = ctx.int("dex.quote_cpu_max").ok().filter(|n| *n > 0)?;
    let sem = {
        let mut g = LIMIT.lock().unwrap_or_else(|p| p.into_inner());
        match g.as_ref() {
            Some((k, s)) if *k == n => s.clone(),
            _ => {
                let s = Arc::new(Semaphore::new(usize::try_from(n).unwrap_or(usize::MAX)));
                *g = Some((n, s.clone()));
                s
            }
        }
    };
    sem.acquire_owned().await.ok()
}

/// Выполняет работу котировки вне рабочих потоков tokio. Принимает: окружение (настройка предела, текст ошибки),
/// работу (владеет своими данными). Возвращает: её итог.
/// Ошибки: её ошибки; Internal — поток счёта упал (паника) или рантайм остановлен.
pub async fn run<F, T>(ctx: &Ctx, work: F) -> GatewayResult<T>
where
    F: Future<Output = GatewayResult<T>> + Send + 'static,
    T: Send + 'static,
{
    let held = permit(ctx).await;
    let class = urgency::is_set();
    let meter = calc_meter::current();
    let fin = quote_stage::is_final();
    let rt = tokio::runtime::Handle::current();
    let job = tokio::task::spawn_blocking(move || {
        let _held = held;
        rt.block_on(urgency::scope_opt(
            class,
            calc_meter::scope_opt(meter, quote_stage::final_if(fin, work)),
        ))
    });
    job.await.map_err(|e| {
        ctx.err(
            ErrorCode::Internal,
            format!("счёт котировки DEX прерван: {e}"),
        )
    })?
}
