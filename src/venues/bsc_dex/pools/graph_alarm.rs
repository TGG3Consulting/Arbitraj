//! Файл: venues/bsc_dex/pools/graph_alarm.rs
//! Что делает: решение 308 (задача t137) — журнал без спама и события Telegram фонового графа маршрутов:
//! - предохранитель (снимок отстал сверх dex.graph_max_lag_ms): счётчик на КАЖДОЕ срабатывание; запись журнала — одна на
//!   эпизод (и одна при выходе — «снимок снова свежий»); событие dex.graph_stale — при первом срабатывании эпизода и далее
//!   не чаще dex.graph_alert_min_s (не задана — одно на эпизод), каждое отправленное — и записью журнала;
//! - ступень Б (снимок без накопительного обновления: прежние настройки графа, догоняющая сборка, доводка): счётчик на
//!   каждую выдачу, запись журнала на переход;
//! - расхождение самопроверки: Error с перечнем отличий + dex.graph_mismatch; падение/зависание владельца: Error +
//!   dex.graph_owner_failed (не чаще dex.graph_alert_min_s); снимка нет из-за настройки: запись на смену причины; сбой
//!   полной пересборки: Error.
//!
//! Вид записей журнала — dex.graph. Запись и отправка — отдельной задачей tokio (вызывают и потоки счёта котировки);
//! вне среды tokio — без записи. Канал ядру полон — отправка отдельной задачей; канала нет — запись «не ушло».
//! Кто вызывает: graph_read.rs, graph_owner.rs, graph_full.rs, graph_guard.rs.
//! От чего зависит: graph_snap.rs (Bg: out, stats), ctx.rs, gateway/events.rs (MarketEvent::Notice), storage/journal.
//! Настройки: dex.graph_alert_min_s [bsc_dex] (необязательная); включение событий — notify.event.* (экран «Уведомления»).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::graph_snap::Bg;

/// Вид записей журнала.
pub const KIND: &str = "dex.graph";
/// Событие: снимок графа отстал сверх предела — маршрутов графа нет.
pub const CODE_STALE: &str = "dex.graph_stale";
/// Событие: самопроверка нашла расхождение — снимок заменён собранным с нуля.
pub const CODE_MISMATCH: &str = "dex.graph_mismatch";
/// Событие: фоновая задача графа упала или зависла — перезапущена.
pub const CODE_OWNER: &str = "dex.graph_owner_failed";

/// Состояние журнала и уведомлений (в Bg).
#[derive(Default)]
pub struct Alarm {
    in_fuse: AtomicBool,
    stage_b: AtomicBool,
    /// Повторное ревью п. 4: с какого момента котировки видят ступень Б (для предела и экрана).
    stage_b_since: Mutex<Option<Instant>>,
    last_sent: Mutex<Option<Instant>>,
    pub(super) owner_sent: Mutex<Option<Instant>>,
    pub(super) full_said: Mutex<Option<(String, Instant)>>,
    pub(super) why_logged: Mutex<Option<String>>,
}

impl Alarm {
    /// Начать отсчёт ступени Б заново (сторож после перезапуска: у нового владельца свой срок). Ошибок нет.
    pub fn restart_stage_b(&self) {
        let mut s = self.stage_b_since.lock().unwrap_or_else(|p| p.into_inner());
        if s.is_some() {
            *s = Some(Instant::now());
        }
    }

    /// Сколько котировки видят ступень Б (None — не видят). Ошибок нет.
    pub fn stage_b_age(&self) -> Option<Duration> {
        self.stage_b_since
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .map(|t| t.elapsed())
    }
}

pub use super::graph_alarm_owner::{full_failed, full_ok, not_built, owner_failed};

/// Запись журнала и (если задан код) событие ядру — отдельной задачей. Ошибок нет.
pub(super) fn emit(
    ctx: &Ctx,
    bg: &Bg,
    level: Level,
    text: String,
    data: Option<Value>,
    code: Option<&str>,
) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let (ctx, out, code) = (ctx.clone(), bg.out(), code.map(str::to_string));
    rt.spawn(async move {
        ctx.log(level, KIND, text.clone(), data).await;
        let Some(code) = code else { return };
        let ev = MarketEvent::Notice {
            venue: ctx.venue(),
            code: code.clone(),
            text,
        };
        let sent = match out {
            Some(o) => o.send(ev).await.is_ok(),
            None => false,
        };
        if !sent {
            let msg =
                format!("Уведомление {code} не ушло — канал событий ядру не открыт или закрыт");
            ctx.log(Level::Warn, KIND, msg, None).await;
        }
    });
}

/// Предохранитель сработал. Принимает: окружение, состояние, отставание, предел, версию снимка и реестра. Ошибок нет.
pub fn fuse(ctx: &Ctx, bg: &Bg, lag: Duration, limit: Duration, snap_v: u64, reg_v: u64) {
    bg.stats.fuse();
    let first = !bg.alarm.in_fuse.swap(true, Ordering::AcqRel);
    if first {
        bg.stats.fuse_episode();
    }
    let repeat = ctx
        .int("dex.graph_alert_min_s")
        .ok()
        .map(Duration::from_secs);
    let send = {
        let mut last = bg.alarm.last_sent.lock().unwrap_or_else(|p| p.into_inner());
        let due = first || matches!((repeat, *last), (Some(r), Some(t)) if t.elapsed() >= r);
        if due {
            *last = Some(Instant::now());
        }
        due
    };
    if !send {
        return;
    }
    let text = format!(
        "Снимок графа маршрутов отстал на {} мс (предел dex.graph_max_lag_ms = {} мс; снимок версии {snap_v}, реестр — {reg_v}, \
         изменённых пулов ждут круга: {}) — маршрутов графа нет, котировки по классике. Причина — фоновая задача графа не \
         успевает или стоит (см. записи dex.graph и dex.graph_build)",
        lag.as_millis(),
        limit.as_millis(),
        bg.changes.pending()
    );
    let data = json!({"lag_ms": lag.as_millis() as u64, "limit_ms": limit.as_millis() as u64, "snapshot_version": snap_v, "registry_version": reg_v});
    emit(ctx, bg, Level::Warn, text, Some(data), Some(CODE_STALE));
}

/// Снимок снова в пределе — конец эпизода предохранителя (одна запись). Ошибок нет.
pub fn fresh_again(ctx: &Ctx, bg: &Bg) {
    if bg.alarm.in_fuse.swap(false, Ordering::AcqRel) {
        let text =
            "Снимок графа маршрутов снова в пределе dex.graph_max_lag_ms — маршруты графа работают"
                .to_string();
        emit(ctx, bg, Level::Info, text, None, None);
    }
}

/// Ступень Б (ревью №2 A, B): `why` — почему отдан снимок без накопительного обновления (прежние настройки графа,
/// догоняющая полная сборка / доводка) — счётчик на каждую выдачу, запись журнала на переход; None — ступень кончилась.
/// Ошибок нет.
pub fn stage_b(ctx: &Ctx, bg: &Bg, why: Option<&str>) {
    if let Some(why) = why {
        bg.stats.stale_settings();
        if !bg.alarm.stage_b.swap(true, Ordering::AcqRel) {
            *bg.alarm
                .stage_b_since
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
            let text = format!(
                "Граф маршрутов: {why} — котировки берут действующий снимок (отставшим он не считается); цена — по \
                 свежему состоянию пулов"
            );
            emit(ctx, bg, Level::Info, text, None, None);
        }
    } else if bg.alarm.stage_b.swap(false, Ordering::AcqRel) {
        let took = bg
            .alarm
            .stage_b_since
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .map_or(0, |t| t.elapsed().as_millis());
        let text = format!(
            "Граф маршрутов: снимок снова обновляется накопительно (ступень Б длилась {took} мс)"
        );
        emit(ctx, bg, Level::Info, text, None, None);
    }
}

/// Самопроверка нашла расхождение (снимок уже заменён собранным с нуля). Принимает: причину пересборки, отличия.
/// Ошибок нет.
pub fn mismatch(ctx: &Ctx, bg: &Bg, reason: &str, lines: &[String]) {
    let head = format!(
        "Самопроверка графа маршрутов ({reason}): накопительная карта разошлась с собранной с нуля — {} отличий; \
         действующим стал снимок, собранный с нуля",
        lines.len()
    );
    bg.stats.mismatch(lines.join("\n"));
    let data = json!({"differences": lines});
    let text = match lines.first() {
        Some(l) => format!("{head}. Первое: {l}"),
        None => head,
    };
    emit(ctx, bg, Level::Error, text, Some(data), Some(CODE_MISMATCH));
}

/// Информационная запись (пересборка по кнопке и т. п.). Ошибок нет.
pub fn info(ctx: &Ctx, bg: &Bg, text: String) {
    emit(ctx, bg, Level::Info, text, None, None);
}
