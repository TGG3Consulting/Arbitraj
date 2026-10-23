//! Файл: api/dex.rs
//! Что делает: списки адаптера DEX для интерфейса — DEX и виды пулов (id области «DEX») и каналы
//! отправки (id области «канал») со статистикой. Данные — только из реестра шлюза (контракт адаптера);
//! к адаптерам напрямую api не ходит.
//! Кто вызывает: интерфейс (экраны «Площадки», «Настройки», «Каналы отправки»).
//! От чего зависит: api/state.rs, gateway/registry.rs, gateway/contract.rs, core/types (send.rs).
//! Настройки: нет (настройки областей DEX и канал меняются обычным PUT /api/settings).
//!
//! GET /api/venues/{id}/dex-families — [DexFamily]; биржа — 400 «не применимо»
//! GET /api/channels/{venue}         — [{info: SendChannelInfo, stats: SendChannelStats | null}]
//!
//! Статистику каналов (решение 42) ведёт ядро по событиям адаптера. Пока ядро её не отдаёт,
//! stats = null — интерфейс показывает «нет данных». Чисел из головы нет.

use axum::Json;
use axum::extract::{Path, State};
use serde::Serialize;

use crate::core::types::{SendChannelInfo, SendChannelStats, VenueId};
use crate::gateway::contract::DexFamily;

use super::state::{ApiError, AppState};

/// Канал отправки для интерфейса: описание от адаптера и последняя статистика от ядра.
#[derive(Serialize)]
pub struct ChannelView {
    pub info: SendChannelInfo,
    /// None — ядро ещё не прислало статистику этого канала (нет данных).
    pub stats: Option<SendChannelStats>,
}

/// DEX и виды пулов площадки (шлюз `dex_families`).
/// Принимает: id площадки. Возвращает: список видов пулов.
/// Ошибки: 400 — не DEX (NotApplicable); 404 — площадка не подключена; 502 — сбой адаптера.
pub async fn families(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<DexFamily>>, ApiError> {
    let venue = st
        .registry
        .get(&VenueId(id))
        .await
        .map_err(ApiError::gateway)?;
    venue
        .dex_families()
        .await
        .map(Json)
        .map_err(ApiError::gateway)
}

/// Каналы отправки площадки (шлюз `send_channels`) со статистикой.
/// Принимает: id площадки. Возвращает: каналы; stats = null, пока ядро не отдаёт статистику.
/// Ошибки: 400 — у площадки нет каналов (биржа); 404 — площадка не подключена; 502 — сбой адаптера.
pub async fn channels(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<ChannelView>>, ApiError> {
    let venue = st
        .registry
        .get(&VenueId(id))
        .await
        .map_err(ApiError::gateway)?;
    let list = venue.send_channels().await.map_err(ApiError::gateway)?;
    // Источника статистики (ядро, core/types/send.rs → SendChannelStats) в api пока нет:
    // честно отдаём null, а не нули.
    let views = list
        .into_iter()
        .map(|info| ChannelView { info, stats: None })
        .collect();
    Ok(Json(views))
}
