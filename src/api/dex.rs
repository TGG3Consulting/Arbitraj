//! Файл: api/dex.rs
//! Что делает: списки адаптера DEX для интерфейса — DEX и виды пулов (id области «DEX») и каналы
//! отправки (id области «канал») со статистикой. Данные — только из реестра шлюза (контракт адаптера);
//! к адаптерам напрямую api не ходит.
//! Кто вызывает: интерфейс (экраны «Площадки», «Настройки», «Каналы отправки»).
//! От чего зависит: api/state.rs, gateway/registry.rs, gateway/contract.rs, core/types (send.rs).
//! Настройки: нет (настройки областей DEX и канал меняются обычным PUT /api/settings).
//!
//! GET /api/venues/{id}/dex-families — [DexFamily]; биржа — 400 «не применимо»
//! GET /api/venues/{id}/pool-directory — PoolDirectory (справочник пулов от индексатора, решение 58); биржа — 400
//! GET /api/venues/{id}/dex-cycles   — [DexCycle] (сигналы циклов DEX ↔ DEX, решение 85; за dex_dex.enabled); биржа — 400
//! GET /api/channels/{venue}         — [{info: SendChannelInfo, stats: SendChannelStats | null}]
//!
//! Статистику каналов (решение 42) ведёт ядро по событиям адаптера (Core::channel_stats). Пока событий
//! канала не было — stats = null, интерфейс показывает «нет данных». Чисел из головы нет.

use axum::Json;
use axum::extract::{Path, State};
use serde::Serialize;

use crate::core::types::{DexCycle, PoolDirectory, SendChannelInfo, SendChannelStats, VenueId};
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

/// Справочник пулов площадки (шлюз `pool_directory`, решение 58).
/// Принимает: id площадки. Возвращает: DEX индексатора, монеты, пулы с итогом адаптера.
/// Ошибки: 400 — не DEX (NotApplicable); 404 — площадка не подключена; 502 — сбой адаптера.
pub async fn directory(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PoolDirectory>, ApiError> {
    let venue = st
        .registry
        .get(&VenueId(id))
        .await
        .map_err(ApiError::gateway)?;
    venue
        .pool_directory()
        .await
        .map(Json)
        .map_err(ApiError::gateway)
}

/// Сигналы циклов DEX ↔ DEX площадки (шлюз `dex_cycles`, решение 85).
/// Принимает: id площадки. Возвращает: сигналы циклов (прибыльные впереди); пусто — направление выключено
/// (dex_dex.enabled) или циклов нет.
/// Ошибки: 400 — не DEX (NotApplicable); 404 — площадка не подключена; 502 — сбой адаптера (узел, настройки).
pub async fn cycles(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<DexCycle>>, ApiError> {
    let venue = st
        .registry
        .get(&VenueId(id))
        .await
        .map_err(ApiError::gateway)?;
    venue
        .dex_cycles()
        .await
        .map(Json)
        .map_err(ApiError::gateway)
}

/// Каналы отправки площадки (шлюз `send_channels`) со статистикой.
/// Принимает: id площадки. Возвращает: каналы; stats = null, пока по каналу не было событий.
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
    // Статистика — из ядра (последние события SendChannel адаптера); нет события — null, а не нули.
    let stats = st.core.channel_stats();
    let views = list
        .into_iter()
        .map(|info| {
            let s = stats.iter().find(|s| s.id == info.id).cloned();
            ChannelView { info, stats: s }
        })
        .collect();
    Ok(Json(views))
}
