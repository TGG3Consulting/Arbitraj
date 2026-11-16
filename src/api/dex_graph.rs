//! Файл: api/dex_graph.rs
//! Что делает: решение 308 (задача t137) — фоновый граф маршрутов DEX для экрана «Площадки» (блок «Граф маршрутов
//! (фон)»): состояние (снимок, отставание, круг, полная пересборка со сверкой, расхождения, предохранитель за час/сутки,
//! перезапуски) и команда «Пересобрать граф». Данные — только через шлюз (контракт адаптера), к адаптеру напрямую api
//! не ходит.
//! Кто вызывает: интерфейс (ui/src/components/DexGraph.svelte).
//! От чего зависит: api/state.rs, gateway/registry.rs, gateway/contract.rs (dex_graph_state, dex_graph_rebuild),
//! core/types (DexGraphState).
//! Настройки: нет (режим и пределы — dex.graph_* обычным PUT /api/settings).
//!
//! GET  /api/venues/{id}/dex-graph         — DexGraphState; биржа — 400 «не применимо»
//! POST /api/venues/{id}/dex-graph/rebuild — полная пересборка со сверкой (действующий снимок работает до замены);
//! 400 — биржа; 404 — площадка не подключена / DEX не подписан на рынок; 502 — режим dex.graph_background выключен
//! (NotConfigured, текст — причина).

use axum::Json;
use axum::extract::{Path, State};

use crate::core::types::{DexGraphState, VenueId};

use super::state::{ApiError, AppState};

/// Состояние фонового графа площадки. Принимает: id площадки. Возвращает: DexGraphState.
/// Ошибки: 400 — не DEX; 404 — площадка не подключена.
pub async fn state(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DexGraphState>, ApiError> {
    let venue = st
        .registry
        .get(&VenueId(id))
        .await
        .map_err(ApiError::gateway)?;
    venue
        .dex_graph_state()
        .await
        .map(Json)
        .map_err(ApiError::gateway)
}

/// «Пересобрать граф»: полная пересборка со сверкой. Принимает: id площадки. Возвращает: новое состояние.
/// Ошибки: 400 — не DEX; 404 — площадка не подключена или DEX не подписан на рынок; 502 — режим выключен (NotConfigured).
pub async fn rebuild(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DexGraphState>, ApiError> {
    let venue = st
        .registry
        .get(&VenueId(id))
        .await
        .map_err(ApiError::gateway)?;
    venue
        .dex_graph_rebuild("вручную, интерфейс")
        .await
        .map_err(ApiError::gateway)?;
    venue
        .dex_graph_state()
        .await
        .map(Json)
        .map_err(ApiError::gateway)
}
