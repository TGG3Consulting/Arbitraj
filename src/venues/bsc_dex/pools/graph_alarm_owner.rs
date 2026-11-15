//! Файл: venues/bsc_dex/pools/graph_alarm_owner.rs
//! Что делает: решение 308 (задача t137) — журнал и уведомления о самом фоновом владельце графа, без повторов
//! (вынесено из graph_alarm.rs: файл сверх 200 строк):
//! - owner_failed — владелец упал/завис/ступень Б сверх предела и перезапущен: Error и dex.graph_owner_failed — не чаще
//!   dex.graph_alert_min_s (не задана — на каждый перезапуск); пропущенная запись не теряет счёт (экран, graph_stats.rs);
//! - full_failed / full_ok — полная пересборка не удалась: Error с той же причиной (ключ причины без изменчивых частей,
//!   например номера задачи у упавшего потока — повторное ревью б) — снова не раньше dex.graph_alert_min_s (не задана —
//!   только при смене причины); удачная сборка сбрасывает память причины;
//! - not_built — снимка нет из-за настройки (котировки по классике) или временный сбой (снимок отдаётся, повтор): запись на
//!   смену причины.
//!
//! Кто вызывает: graph_alarm.rs (реэкспорт), graph_step.rs, graph_finish.rs, graph_guard.rs.
//! От чего зависит: graph_alarm.rs (Alarm, emit, KIND, CODE_OWNER), graph_snap.rs (Bg), ctx.rs, storage/journal.
//! Настройки: dex.graph_alert_min_s [bsc_dex] (необязательная).

use std::time::{Duration, Instant};

use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::graph_alarm::{CODE_OWNER, emit};
use super::graph_snap::Bg;

/// Срок повтора dex.graph_alert_min_s. Ошибок нет.
fn repeat(ctx: &Ctx) -> Option<Duration> {
    ctx.int("dex.graph_alert_min_s")
        .ok()
        .map(Duration::from_secs)
}

/// Владелец перезапущен (см. шапку). Ошибок нет.
pub fn owner_failed(ctx: &Ctx, bg: &Bg, why: &str) {
    let due = {
        let mut last = bg
            .alarm
            .owner_sent
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let due = match (repeat(ctx), *last) {
            (Some(r), Some(t)) => t.elapsed() >= r,
            _ => true,
        };
        if due {
            *last = Some(Instant::now());
        }
        due
    };
    if !due {
        return;
    }
    let text = format!(
        "Фоновая задача графа маршрутов перезапущена: {why}. До нового снимка котировки берут действующий снимок \
         (идёт полная пересборка; цена — по свежему состоянию пулов)"
    );
    emit(ctx, bg, Level::Error, text, None, Some(CODE_OWNER));
}

/// Полная пересборка не удалась. Принимает: причину пересборки, ключ причины (без изменчивых частей) и текст. Ошибок нет.
pub fn full_failed(ctx: &Ctx, bg: &Bg, reason: &str, key: &str, why: &str) {
    {
        let mut last = bg.alarm.full_said.lock().unwrap_or_else(|p| p.into_inner());
        let due = match (&*last, repeat(ctx)) {
            (Some((k, _)), _) if k != key => true,
            (Some((_, t)), Some(r)) => t.elapsed() >= r,
            (Some(_), None) => false,
            (None, _) => true,
        };
        if !due {
            return;
        }
        *last = Some((key.to_string(), Instant::now()));
    }
    let text = format!(
        "Полная пересборка графа маршрутов ({reason}) не удалась: {why}. Действующий снимок остаётся"
    );
    emit(ctx, bg, Level::Error, text, None, None);
}

/// Полная пересборка удалась — память причины сбоя сброшена (следующий сбой будет записан). Ошибок нет.
pub fn full_ok(bg: &Bg) {
    *bg.alarm.full_said.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

/// Снимка нет или временный сбой — запись на смену причины (None — причина ушла). `transient` — временный сбой: снимок
/// отдаётся, повтор (повторное ревью г). Ошибок нет.
pub fn not_built(ctx: &Ctx, bg: &Bg, why: Option<&str>, transient: bool) {
    let mut w = bg
        .alarm
        .why_logged
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if w.as_deref() == why {
        return;
    }
    *w = why.map(str::to_string);
    let Some(why) = why else { return };
    let text = if transient {
        format!(
            "Граф маршрутов (фон): временный сбой — {why}; действующий снимок отдаётся, повтор на следующем изменении"
        )
    } else {
        format!(
            "Граф маршрутов (фон) не собирается: {why} — маршрутов графа нет, котировки по классике"
        )
    };
    emit(ctx, bg, Level::Warn, text, None, None);
}
