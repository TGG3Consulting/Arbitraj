//! Файл: api/tip_share.rs
//! Что делает: решение 437 (3) — доля прибыли на чаевые сделок DEX ↔ DEX площадки (сеть eip1559_l1, режим
//! send.tip_mode = profit_share) для экрана «Каналы отправки»: текущая доля, последние чаевые, счётчики, последний
//! проигрыш (перехватчик и его чаевые), кнопка «Сбросить долю к минимуму». Данные — только через шлюз (контракт
//! адаптера tip_share, reset_tip_share), к адаптеру напрямую api не ходит.
//! Кто вызывает: интерфейс (ui/src/components/TipShare.svelte).
//! От чего зависит: api/state.rs, gateway/registry.rs, gateway/contract.rs, core/types (TipShare).
//! Настройки: нет (режим и пределы — send.tip_* обычным PUT /api/settings).
//!
//! GET  /api/venues/{id}/tip-share       — TipShare; биржа — 400 «не применимо»
//! POST /api/venues/{id}/tip-share/reset — сброс доли к send.tip_share_min_pct; 400 — биржа; 404 — нет площадки

use axum::Json;
use axum::extract::{Path, State};

use crate::core::types::{TipShare, VenueId};

use super::state::{ApiError, AppState};

/// Доля чаевых площадки. Принимает: id площадки. Возвращает: TipShare.
/// Ошибки: 400 — не DEX; 404 — площадка не подключена.
pub async fn state(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TipShare>, ApiError> {
    let venue = st
        .registry
        .get(&VenueId(id))
        .await
        .map_err(ApiError::gateway)?;
    venue.tip_share().await.map(Json).map_err(ApiError::gateway)
}

/// «Сбросить долю к минимуму». Принимает: id площадки. Возвращает: состояние после сброса.
/// Ошибки: 400 — не DEX; 404 — площадка не подключена.
pub async fn reset(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TipShare>, ApiError> {
    let venue = st
        .registry
        .get(&VenueId(id))
        .await
        .map_err(ApiError::gateway)?;
    venue
        .reset_tip_share("вручную, интерфейс")
        .await
        .map(Json)
        .map_err(ApiError::gateway)
}
