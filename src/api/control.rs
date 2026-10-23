//! Файл: api/control.rs
//! Что делает: экраны «Ручное управление» (застрявшие монеты, «Беру на себя», «Вернуть боту», решение 37)
//! и «Статистика газа» (по монетам, решение 36).
//! Кто вызывает: интерфейс (экраны «Ручное управление», «Статистика газа»).
//! От чего зависит: api/state.rs, core/types (ManualAction), storage/journal.rs.
//! Настройки: нет (режим застрявших монет — stuck.*, экран «Настройки»).
//!
//! GET  /api/manual          — {alerts: [StuckAlert], holdings: [ManualHolding]}
//! POST /api/manual/command  — {action: ManualAction} → {ok, details}
//! GET  /api/gas-stats       — [CoinGasStats]
//!
//! Данные и исполнение команд — в ядре (core/stuck.rs, core/manual.rs, core/commands.rs, core/gas_stats.rs).
//! Ядро к веб-серверу ещё не подключено: чтение отвечает 503 «нет данных», команда не выполняется
//! (503 с причиной, попытка пишется в журнал). Пустых списков и нулей вместо данных нет.

use axum::Json;
use axum::extract::State;
use serde::Deserialize;

use crate::core::types::ManualAction;
use crate::storage::journal::Level;

use super::state::{ApiError, AppState};

/// Почему данных нет — одна фраза для всех ответов этого файла.
const NO_CORE: &str = "ядро ещё не подключено к веб-серверу";

/// Алерты застрявших монет и монеты «Беру на себя». Ошибки: 503 — нет данных.
pub async fn manual() -> ApiError {
    ApiError::no_data(format!(
        "Нет данных: застрявшие монеты и ручное управление ведёт ядро (core/stuck.rs, core/manual.rs) — {NO_CORE}"
    ))
}

/// Тело команды.
#[derive(Deserialize)]
pub struct CommandBody {
    action: ManualAction,
}

/// Команда человека из интерфейса (источник — CommandSource::Ui).
/// Принимает: действие. Возвращает: {ok, details} от ядра.
/// Ошибки: 503 — ядро не подключено, команда не выполнена (попытка пишется в журнал);
/// 400/422 — неверное тело (разбор axum).
pub async fn command(
    State(st): State<AppState>,
    Json(b): Json<CommandBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let what = match &b.action {
        ManualAction::SellNow { alert_id } => format!("«Продать сейчас» по алерту {alert_id}"),
        ManualAction::TakeOver { alert_id } => format!("«Беру на себя» по алерту {alert_id}"),
        ManualAction::ReturnToBot { holding_id } => format!("«Вернуть боту» для {holding_id}"),
    };
    let msg = format!("Команда из интерфейса {what} не выполнена: {NO_CORE}");
    let _ = st
        .journal
        .write(Level::Warn, "manual.command", None, msg.clone(), None)
        .await;
    Err(ApiError::no_data(msg))
}

/// Статистика газа по монетам. Ошибки: 503 — нет данных.
pub async fn gas_stats() -> ApiError {
    ApiError::no_data(format!(
        "Нет данных: статистику газа по монетам ведёт ядро (core/gas_stats.rs) — {NO_CORE}"
    ))
}
