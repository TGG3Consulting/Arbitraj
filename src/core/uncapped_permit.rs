//! Файл: core/uncapped_permit.rs
//! Что делает: место под расчёт «без потолка» (решения 141, 197): семафор uncapped.max_concurrent (пересоздаётся при
//! смене значения) и приоритет сделки — кандидат берёт место, только когда не ждёт ни одна сделка (State::deals_waiting);
//! получив место при ждущих сделках, кандидат его отдаёт (место уходит следующему в очереди семафора — сделке или
//! кандидату, который тоже отдаст) и ждёт State::turn. Так сделка не стоит за десятками кандидатов.
//! Кто вызывает: core/uncapped_queue.rs (run).
//! От чего зависит: uncapped_state.rs, engine.rs, settings, tokio (Semaphore, Notify).
//! Настройки: uncapped.max_concurrent.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::settings::read::At;

use super::engine::{Core, lk};

/// Место под расчёт по uncapped.max_concurrent; кандидат — только когда не ждёт ни одна сделка (решение 197): получив
/// место при ждущих сделках, кандидат отдаёт его (место уходит следующему в очереди семафора) и ждёт turn.
/// Принимает: ядро, сделка ли. Ошибки: текст — uncapped.max_concurrent не задана или 0.
pub async fn permit(core: &Core, deal: bool) -> Result<OwnedSemaphorePermit, String> {
    let st = &core.inner.uncapped;
    loop {
        if !deal {
            let turn = st.turn.notified();
            tokio::pin!(turn);
            turn.as_mut().enable();
            if st.deals_waiting.load(Ordering::Acquire) > 0 {
                turn.await;
                continue;
            }
        }
        let held = slot(core).await?;
        if deal || st.deals_waiting.load(Ordering::Acquire) == 0 {
            return Ok(held);
        }
    }
}

/// Одно место семафора uncapped.max_concurrent. Ошибки: текст — не задана или 0.
async fn slot(core: &Core) -> Result<OwnedSemaphorePermit, String> {
    let n = core
        .inner
        .deps
        .settings
        .integer("uncapped.max_concurrent", At::default())
        .map_err(|e| e.message)?;
    if n == 0 {
        return Err("uncapped.max_concurrent = 0: расчёты без потолка не идут".into());
    }
    let sem = {
        let mut g = lk(&core.inner.uncapped.sem);
        match g.as_ref() {
            Some((k, s)) if *k == n => s.clone(),
            _ => {
                let s = Arc::new(Semaphore::new(usize::try_from(n).unwrap_or(usize::MAX)));
                *g = Some((n, s.clone()));
                s
            }
        }
    };
    sem.acquire_owned().await.map_err(|e| e.to_string())
}
