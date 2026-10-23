//! Файл: api/venues.rs
//! Что делает: список подключённых площадок — идентификаторы из реестра шлюза.
//! Кто вызывает: интерфейс (экраны «Площадки», «Настройки», «Правила шлюза», «Уведомления»).
//! От чего зависит: api/state.rs, gateway/registry.rs.
//! Настройки: нет.
//!
//! GET /api/venues — ["bingx", …], по алфавиту

use axum::Json;
use axum::extract::State;

use crate::core::types::VenueId;

use super::state::AppState;

/// Идентификаторы площадок, зарегистрированных в шлюзе (по алфавиту). Ошибок нет.
pub async fn list(State(st): State<AppState>) -> Json<Vec<VenueId>> {
    let mut ids = st.registry.ids().await;
    ids.sort_by(|a, b| a.0.cmp(&b.0));
    Json(ids)
}
