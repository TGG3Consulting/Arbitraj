//! Файл: api/wallet_queue.rs
//! Что делает: очередь транзакций кошелька DEX для интерфейса (решение 45) — состояние (свободна / ждёт итога /
//! остановлена до ручной проверки с причиной) и кнопка «Снять очередь после ручной проверки». Данные — только из
//! реестра шлюза (контракт адаптера wallet_queue, release_wallet_queue); к адаптерам напрямую api не ходит.
//! Кто вызывает: интерфейс (экран «Каналы отправки», components/WalletQueue.svelte).
//! От чего зависит: api/state.rs, gateway/registry.rs, gateway/contract.rs, core/types (WalletQueue).
//! Настройки: нет.
//!
//! GET  /api/wallet-queue/{venue}          — WalletQueue; биржа — 400 «не применимо»
//! GET отдаёт и nonce_taken: у остановленной очереди — занят ли её nonce в блоке (false — транзакция может пройти позже).
//! POST /api/wallet-queue/{venue}/release {pending_ok} — WalletQueue после снятия; очередь не остановлена — 400;
//!   nonce ещё не занят и pending_ok = false — 400 (NonceConflict), интерфейс просит второе подтверждение

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;

use crate::core::types::{VenueId, WalletQueue};

use super::state::{ApiError, AppState};

/// Состояние очереди кошелька площадки. Ошибки: 400 — не DEX; 404 — площадка не подключена; 502 — сбой адаптера.
pub async fn get(
    State(st): State<AppState>,
    Path(venue): Path<String>,
) -> Result<Json<WalletQueue>, ApiError> {
    let v = st
        .registry
        .get(&VenueId(venue))
        .await
        .map_err(ApiError::gateway)?;
    v.wallet_queue().await.map(Json).map_err(ApiError::gateway)
}

/// Тело запроса снятия.
#[derive(Deserialize)]
pub struct Release {
    /// Снять, хотя nonce остановленной транзакции ещё не занят (второй шаг подтверждения в интерфейсе).
    #[serde(default)]
    pub pending_ok: bool,
}

/// «Снять очередь после ручной проверки». Принимает: {pending_ok}. Возвращает: новое состояние.
/// Ошибки: 400 — очередь не остановлена, не DEX; nonce ещё не занят, а pending_ok нет (код NonceConflict — текст
/// для второго подтверждения); 404 — площадка не подключена.
pub async fn release(
    State(st): State<AppState>,
    Path(venue): Path<String>,
    Json(body): Json<Release>,
) -> Result<Json<WalletQueue>, ApiError> {
    let v = st
        .registry
        .get(&VenueId(venue))
        .await
        .map_err(ApiError::gateway)?;
    v.release_wallet_queue("интерфейс", body.pending_ok)
        .await
        .map(Json)
        .map_err(ApiError::gateway)
}
