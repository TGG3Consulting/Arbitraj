//! Файл: venues/bsc_dex/pools/lost_events.rs
//! Что делает: решение 313 — счётчик событий MarketEvent::Pool, не отправленных ядру из-за полного канала (listen_log.rs
//! emit шлёт без ожидания, чтобы медленное ядро не задерживало чтение событий; при полном канале событие пула раньше
//! терялось молча — свежий пул лежит в реестре, но ядро о нём не узнаёт до следующего события пула). Счётчик ОБЩИЙ на
//! Signals подписки на рынок (pools/progress.rs; все пачки и слушатели этой подписки), а не одной пачки. Каждая потеря
//! считается; запись журнала уровня Warn `dex.pool_events_lost` с числом потерь с прошлой записи — не чаще раза в
//! log.pool_events_lost_interval_s секунд (первая — сразу). Не задана — записей нет (только счёт в памяти), поведение
//! отправки прежнее.
//! Ревью 313 (горячий путь): настройка интервала читается без построения текста ошибки (Store::resolve, а не ctx.int)
//! и, пока она задана, — только когда по прочитанному в прошлый раз интервалу запись уже могла бы быть пора (Clock::due);
//! новое значение интервала действует с ближайшей такой проверки.
//! Решение 321: при dex.pool_events_keep = да события не теряются (emit_pool.rs, gateway/keep_last.rs) — та же запись
//! dex.pool_events_lost (тот же интервал) пишет счёт набора «отложено, заменено, доставлено, снято, ждут» (kept).
//! Решение 324: набор — общий на канал ядра (gateway/core_out.rs), счёт — класса «пулы этой площадки»; часы записи
//! (Clock) перенесены в gateway/log_clock.rs (здесь — реэкспорт).
//! Кто вызывает: pools/emit_pool.rs (emit — note, kept; доставщик — kept).
//! От чего зависит: ctx.rs (настройка, журнал), gateway/core_out.rs (счёт набора), gateway/log_clock.rs, tokio
//! (запись отдельной задачей).
//! Настройки: log.pool_events_lost_interval_s (вся система; необязательная).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::gateway::core_out::Feed;
use crate::gateway::core_out_item::Class;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;

/// Часы записи — общие со стаканами бирж (решение 324: перенесены в шлюз; прежний путь сохранён).
pub use crate::gateway::log_clock::Clock;

/// Имя настройки интервала записи.
const INTERVAL: &str = "log.pool_events_lost_interval_s";

/// Счётчик потерь подписки на рынок (общий на её Signals). Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Lost {
    /// Потерь с прошлой записи журнала.
    count: Arc<AtomicU64>,
    /// Когда была прошлая запись и какой интервал прочитан.
    clock: Arc<Mutex<Clock>>,
}

impl Lost {
    /// +1 потеря без записи журнала (чистая логика). Ошибок нет.
    pub fn add(&self) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Потерь с прошлой записи. Ошибок нет.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Событие пула потеряно (канал ядра полон): +1 к счёту; задан интервал и он прошёл с прошлой записи — запись
    /// журнала с числом потерь (счёт обнуляется). Принимает: окружение. Ошибок нет.
    pub fn note(&self, ctx: &Ctx) {
        self.add();
        let Some(since) = self.due(ctx) else {
            return;
        };
        let n = self.count.swap(0, Ordering::Relaxed);
        let msg = format!(
            "события пулов не дошли до ядра: канал событий ядра полон — потеряно {n} {} (свежее состояние пула \
             в реестре есть, но ядро узнает о пуле только со следующим его событием)",
            window(since)
        );
        let data = json!({ "lost": n, "since_s": since.map(|d| d.as_secs()) });
        write(ctx, msg, data);
    }

    /// Решение 321 (dex.pool_events_keep = да): событие отложено или набор опустел — если пора, запись того же вида
    /// dex.pool_events_lost со счётом набора: отложено, заменено, доставлено, снято, ждут (и потеряно — если настройку
    /// включили между потерями). Счёт — класса «пулы этой площадки» общего набора канала ядра (решение 324,
    /// gateway/core_out.rs). Принимает: окружение, общий набор. Ошибок нет.
    pub fn kept(&self, ctx: &Ctx, feed: &Feed) {
        let Some(since) = self.due(ctx) else {
            return;
        };
        let lost = self.count.swap(0, Ordering::Relaxed);
        let c = feed.take(&Class::Pool(ctx.venue()));
        let msg = format!(
            "события пулов: канал событий ядра полон — не потеряны (dex.pool_events_keep): отложено {}, заменено \
             более свежими {}, доставлено {}, снято {} (пул недостоверен или снят), ждут {}, потеряно {lost} {}",
            c.deferred,
            c.replaced,
            c.delivered,
            c.dropped,
            c.waiting,
            window(since)
        );
        let data = json!({
            "lost": lost, "deferred": c.deferred, "replaced": c.replaced, "delivered": c.delivered,
            "dropped": c.dropped, "waiting": c.waiting, "since_s": since.map(|d| d.as_secs()),
        });
        write(ctx, msg, data);
    }

    /// Пора ли запись (Clock::due с чтением интервала из настроек). None — не писать. Ошибок нет.
    pub(super) fn due(&self, ctx: &Ctx) -> Option<Option<Duration>> {
        let mut c = self.clock.lock().unwrap_or_else(|p| p.into_inner());
        c.due(Instant::now(), || interval(ctx))
    }
}

/// log.pool_events_lost_interval_s без текста ошибки: None — не задана (или не число). Ошибок нет.
fn interval(ctx: &Ctx) -> Option<Duration> {
    ctx.settings
        .resolve(INTERVAL, ctx.at().venue, None)
        .and_then(|v| v.as_u64())
        .map(Duration::from_secs)
}

/// «за N с» / «с запуска подписки». Ошибок нет.
pub(super) fn window(since: Option<Duration>) -> String {
    since.map_or("с запуска подписки".to_string(), |d| {
        format!("за {} с", d.as_secs())
    })
}

/// Запись журнала dex.pool_events_lost отдельной задачей (вызывающий не ждёт). Нет рантайма tokio — записи нет.
/// Ошибок нет.
pub(super) fn write(ctx: &Ctx, msg: String, data: serde_json::Value) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let ctx = ctx.clone();
    rt.spawn(async move {
        ctx.log(Level::Warn, "dex.pool_events_lost", msg, Some(data))
            .await;
    });
}
