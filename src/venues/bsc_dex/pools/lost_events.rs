//! Файл: venues/bsc_dex/pools/lost_events.rs
//! Что делает: решение 313 — счётчик событий MarketEvent::Pool, не отправленных ядру из-за полного канала (listen_log.rs
//! emit шлёт try_send, чтобы медленное ядро не задерживало чтение событий; при полном канале событие пула раньше
//! терялось молча — свежий пул лежит в реестре, но ядро о нём не узнаёт до следующего события пула). Каждая потеря
//! считается; запись журнала уровня Warn `dex.pool_events_lost` с числом потерь с прошлой записи — не чаще раза в
//! log.pool_events_lost_interval_s секунд (первая — сразу). Не задана — записей нет (только счёт в памяти), поведение
//! отправки прежнее.
//! Кто вызывает: pools/listen_log.rs (emit — note).
//! От чего зависит: ctx.rs (настройка, журнал), tokio (запись отдельной задачей).
//! Настройки: log.pool_events_lost_interval_s (вся система; необязательная).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::storage::journal::Level;

use super::super::ctx::Ctx;

/// Счётчик потерь одной подписки. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Lost {
    /// Потерь с прошлой записи журнала.
    count: Arc<AtomicU64>,
    /// Когда была прошлая запись (None — ещё не было).
    last: Arc<Mutex<Option<Instant>>>,
}

impl Lost {
    /// Событие пула потеряно (канал ядра полон): +1 к счёту; задан интервал и он прошёл с прошлой записи — запись
    /// журнала с числом потерь (счёт обнуляется). Принимает: окружение. Ошибок нет.
    pub fn note(&self, ctx: &Ctx) {
        self.count.fetch_add(1, Ordering::Relaxed);
        let Ok(every) = ctx.int("log.pool_events_lost_interval_s") else {
            return;
        };
        let since = {
            let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
            let (now, prev) = (Instant::now(), *last);
            if let Some(t) = prev
                && now.duration_since(t) < Duration::from_secs(every)
            {
                return;
            }
            *last = Some(now);
            prev.map(|t| now.duration_since(t))
        };
        let n = self.count.swap(0, Ordering::Relaxed);
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let window = since.map_or("с запуска подписки".to_string(), |d| {
            format!("за {} с", d.as_secs())
        });
        let msg = format!(
            "события пулов не дошли до ядра: канал событий ядра полон — потеряно {n} {window} (свежее состояние пула \
             в реестре есть, но ядро узнает о пуле только со следующим его событием)"
        );
        let data = json!({ "lost": n, "since_s": since.map(|d| d.as_secs()) });
        let ctx = ctx.clone();
        rt.spawn(async move {
            ctx.log(Level::Warn, "dex.pool_events_lost", msg, Some(data))
                .await;
        });
    }
}
