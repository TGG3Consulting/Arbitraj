//! Файл: api/venues.rs
//! Что делает: список подключённых площадок — идентификаторы из реестра шлюза.
//! Кто вызывает: интерфейс (экраны «Площадки», «Настройки», «Правила шлюза», «Уведомления»).
//! От чего зависит: api/state.rs, gateway/registry.rs.
//! Настройки: нет.
//!
//! GET /api/venues       — ["bingx", …], по алфавиту
//! GET /api/venues/kinds — [{id, kind: "Cex" | "Dex"}], по алфавиту — чтобы подписать область
//!                         «площадка-источник перевода» направлением (решение 51)

use axum::Json;
use axum::extract::State;

use serde::Serialize;

use crate::core::types::VenueId;
use crate::gateway::contract::VenueKind;

use super::state::AppState;

/// Идентификаторы площадок, зарегистрированных в шлюзе (по алфавиту). Ошибок нет.
pub async fn list(State(st): State<AppState>) -> Json<Vec<VenueId>> {
    let mut ids = st.registry.ids().await;
    ids.sort_by(|a, b| a.0.cmp(&b.0));
    Json(ids)
}

/// Площадка и её вид.
#[derive(Serialize)]
pub struct VenueInfo {
    pub id: VenueId,
    pub kind: VenueKind,
}

/// Площадки с видом (биржа или DEX), по алфавиту. Площадка, пропавшая из реестра между запросами, пропускается.
pub async fn kinds(State(st): State<AppState>) -> Json<Vec<VenueInfo>> {
    let mut ids = st.registry.ids().await;
    ids.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = Vec::new();
    for id in ids {
        if let Ok(v) = st.registry.get(&id).await {
            out.push(VenueInfo { id, kind: v.kind() });
        }
    }
    Json(out)
}
