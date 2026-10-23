//! Файл: api/control.rs
//! Что делает: экраны «Ручное управление» (застрявшие монеты, «Беру на себя», «Вернуть боту», решение 37)
//! и «Статистика газа» (по монетам, решение 36) — данные и команды ядра.
//! Кто вызывает: интерфейс (экраны «Ручное управление», «Статистика газа»).
//! От чего зависит: api/state.rs, core (Core::manual, Core::command, Core::gas_stats), core/types.
//! Настройки: нет (режим застрявших монет — stuck.*, экран «Настройки»).
//!
//! GET  /api/manual          — {alerts: [StuckAlert], holdings: [ManualHolding]}
//! POST /api/manual/command  — {action: ManualAction} → {ok, details}
//! GET  /api/gas-stats       — [CoinGasStats]

use axum::Json;
use axum::extract::State;
use chrono::Utc;
use serde::Deserialize;

use crate::core::types::{CoinGasStats, CommandSource, ManualAction, ManualCommand};
use crate::gateway::contract::ConnectionCheck;

use super::state::AppState;

/// Алерты застрявших монет и монеты «Беру на себя». Ошибок нет.
pub async fn manual(State(st): State<AppState>) -> Json<serde_json::Value> {
    Json(st.core.manual())
}

/// Тело команды.
#[derive(Deserialize)]
pub struct CommandBody {
    action: ManualAction,
}

/// Команда человека из интерфейса (источник — CommandSource::Ui). Ядро пишет её в журнал.
/// Принимает: действие. Возвращает: {ok, details}. Ошибки: 400/422 — неверное тело (разбор axum).
pub async fn command(
    State(st): State<AppState>,
    Json(b): Json<CommandBody>,
) -> Json<ConnectionCheck> {
    let cmd = ManualCommand {
        action: b.action,
        source: CommandSource::Ui,
        by: None,
        at: Utc::now(),
    };
    Json(match st.core.command(cmd).await {
        Ok(details) => ConnectionCheck { ok: true, details },
        Err(details) => ConnectionCheck { ok: false, details },
    })
}

/// Статистика газа по монетам. Ошибок нет.
pub async fn gas_stats(State(st): State<AppState>) -> Json<Vec<CoinGasStats>> {
    Json(st.core.gas_stats())
}
