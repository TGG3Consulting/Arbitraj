//! Файл: api/dex_executor.rs
//! Что делает: развёртывание своего исполнителя ArbExecutor (решение 122, часть X) для интерфейса — кнопка «Развернуть
//! исполнитель» (Площадки → DEX): оценка (газ, цена, стоимость, будущий адрес, хеш байткода) и отправка транзакции
//! создания по подтверждению человека. Действие с деньгами: отправка — только POST с SHA-256 байткода из оценки,
//! которую человек видел и подтвердил в окне. Сборка и отправка — у адаптера; api только находит площадку.
//! Кто вызывает: интерфейс (components/ExecutorDeploy.svelte).
//! От чего зависит: api/state.rs, gateway/registry.rs, gateway/contract.rs, core/types (executor.rs).
//! Настройки: нет (адрес исполнителя после развёртывания Тигран вписывает в exec.executor_address сам).
//!
//! GET  /api/venues/{id}/executor/quote  → ExecutorDeployQuote (ничего не отправляет); биржа — 400
//! POST /api/venues/{id}/executor/deploy {sha256} → ExecutorDeployed; хеш не тот — 502 (QuoteMismatch); биржа — 400

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;

use crate::core::types::{ExecutorDeployQuote, ExecutorDeployed, VenueId};

use super::state::{ApiError, AppState};

/// Тело подтверждения.
#[derive(Deserialize)]
pub struct DeployReq {
    /// SHA-256 кода создания из оценки, которую подтвердил человек.
    pub sha256: String,
}

/// Оценка развёртывания (шлюз `dex_executor_quote`). Ошибки: 400 — не DEX; 404 — площадка не подключена; 502 — сбой.
pub async fn quote(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ExecutorDeployQuote>, ApiError> {
    let venue = st
        .registry
        .get(&VenueId(id))
        .await
        .map_err(ApiError::gateway)?;
    venue
        .dex_executor_quote()
        .await
        .map(Json)
        .map_err(ApiError::gateway)
}

/// Развёртывание (шлюз `dex_executor_deploy`). Ошибки: 400 — не DEX или пустой хеш; 404; 502 — хеш не тот
/// (QuoteMismatch) или сбой отправки.
pub async fn deploy(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<DeployReq>,
) -> Result<Json<ExecutorDeployed>, ApiError> {
    if b.sha256.trim().is_empty() {
        return Err(ApiError::bad("нет sha256 подтверждённого байткода"));
    }
    let venue = st
        .registry
        .get(&VenueId(id))
        .await
        .map_err(ApiError::gateway)?;
    venue
        .dex_executor_deploy(b.sha256.trim().to_string())
        .await
        .map(Json)
        .map_err(ApiError::gateway)
}
