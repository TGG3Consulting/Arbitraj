//! Файл: api/check.rs
//! Что делает: кнопки «Проверить подключение» — Telegram и площадки.
//! Ответ всегда {ok, details}: «Подключено …» или точная причина сбоя.
//! Каждая проверка пишется в журнал.
//! Кто вызывает: интерфейс (экраны «Уведомления», «Площадки»).
//! От чего зависит: api/state.rs, notify/check.rs, gateway/registry.rs.
//! Настройки: те, что нужны проверяемому подключению.
//!
//! POST /api/check/telegram
//! POST /api/check/venue/{id}

use axum::Json;
use axum::extract::{Path, State};

use crate::core::types::VenueId;
use crate::gateway::contract::ConnectionCheck;
use crate::notify;
use crate::storage::journal::Level;

use super::state::AppState;

/// Проверка Telegram. Ошибок нет: причина сбоя — в details.
pub async fn telegram(State(st): State<AppState>) -> Json<ConnectionCheck> {
    let res = notify::check::telegram(&st.http, &st.settings, &st.vault).await;
    log(&st, None, "Telegram", &res).await;
    Json(res)
}

/// Проверка площадки. Пока адаптеров нет — ответ «площадка не подключена».
/// Ошибок нет: причина сбоя — в details.
pub async fn venue(State(st): State<AppState>, Path(id): Path<String>) -> Json<ConnectionCheck> {
    let res = match st.registry.get(&VenueId(id.clone())).await {
        Ok(v) => v
            .check_connection()
            .await
            .unwrap_or_else(|e| ConnectionCheck {
                ok: false,
                details: e.message,
            }),
        Err(e) => ConnectionCheck {
            ok: false,
            details: e.message,
        },
    };
    log(&st, Some(&id), &id, &res).await;
    Json(res)
}

/// Пишет результат проверки в журнал.
async fn log(st: &AppState, venue: Option<&str>, what: &str, res: &ConnectionCheck) {
    let level = if res.ok { Level::Info } else { Level::Warn };
    let msg = format!("Проверка подключения {what}: {}", res.details);
    let _ = st.journal.write(level, "check", venue, msg, None).await;
}
