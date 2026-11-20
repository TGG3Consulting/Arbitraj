//! Файл: api/pool_cond_stat.rs
//! Что делает: решение 439 — статистика сделок цикла DEX ↔ DEX через исполнитель площадки (сеть eip1559_l1) по
//! режиму условия пулов send.pool_cond_mode на момент отправки (off / pools): отправки, победы, откаты и их газ,
//! отказы по условию пулов и по окну времени; кнопка «Сбросить статистику». Данные — только через шлюз (контракт
//! адаптера pool_cond_stat, reset_pool_cond_stat), к адаптеру напрямую api не ходит.
//! Кто вызывает: интерфейс (ui/src/components/PoolCondStat.svelte).
//! От чего зависит: api/state.rs, gateway/registry.rs, gateway/contract.rs, core/types (PoolCondStat).
//! Настройки: нет (режим — send.pool_cond_mode обычным PUT /api/settings).
//!
//! GET  /api/venues/{id}/pool-cond-stat       — PoolCondStat; не DEX или не eip1559_l1 — 400 «не применимо»
//! POST /api/venues/{id}/pool-cond-stat/reset — сброс счётчиков; 400 — не применимо; 404 — нет площадки

use axum::Json;
use axum::extract::{Path, State};

use crate::core::types::{PoolCondStat, VenueId};

use super::state::{ApiError, AppState};

/// Статистика площадки. Принимает: id площадки. Возвращает: PoolCondStat.
/// Ошибки: 400 — не DEX или не eip1559_l1; 404 — площадка не подключена.
pub async fn state(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PoolCondStat>, ApiError> {
    let venue = st
        .registry
        .get(&VenueId(id))
        .await
        .map_err(ApiError::gateway)?;
    venue
        .pool_cond_stat()
        .await
        .map(Json)
        .map_err(ApiError::gateway)
}

/// «Сбросить статистику». Принимает: id площадки. Возвращает: статистику после сброса.
/// Ошибки: 400 — не DEX или не eip1559_l1; 404 — площадка не подключена.
pub async fn reset(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PoolCondStat>, ApiError> {
    let venue = st
        .registry
        .get(&VenueId(id))
        .await
        .map_err(ApiError::gateway)?;
    venue
        .reset_pool_cond_stat("вручную, интерфейс")
        .await
        .map(Json)
        .map_err(ApiError::gateway)
}
