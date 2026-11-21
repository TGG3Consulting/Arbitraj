//! Файл: api/dex_key.rs
//! Что делает: решение 490 — суффикс сети ключа области «DEX» площадки DEX для интерфейса («Добавить DEX» на экране
//! «Настройки»: человек выбирает площадку DEX и вводит только id, ключ = id + суффикс этой площадки — «@сеть» руками
//! не печатается) и список площадок DEX шлюза для проверки нового ключа «id@сеть» при записи (api/settings.rs set).
//! Правило суффикса — одно, settings/dex_key.rs (его же использует площадка, venues/bsc_dex/ctx_place.rs dex_suffix);
//! «одна ли площадка DEX» — по реестру шлюза: в нём площадки DEX — ровно площадки адаптера DEX (main.rs регистрирует
//! bsc_dex и dex.venues). Стык шлюза (gateway/contract.rs) не меняется: нужен только вид площадки (Venue::kind).
//! Кто вызывает: интерфейс (components/DexAdder.svelte), api/settings.rs (dex_ids).
//! От чего зависит: api/state.rs, gateway/registry.rs, gateway/contract.rs (VenueKind), settings/dex_key.rs.
//! Настройки: dex.chain [площадка DEX], dex.default_chain — только читает (через settings/dex_key.rs).
//!
//! GET /api/venues/{id}/dex-key-suffix — {venue, suffix: null | "@arbitrum"}; биржа — 400; нет площадки — 404;
//!     суффикс не определить (не задана dex.chain / dex.default_chain при нескольких площадках DEX) — 400 с причиной.

use axum::Json;
use axum::extract::{Path, State};
use serde::Serialize;

use crate::core::types::VenueId;
use crate::gateway::contract::VenueKind;
use crate::settings::dex_key::venue_suffix;

use super::state::{ApiError, AppState};

/// Суффикс ключа области «DEX» площадки.
#[derive(Serialize)]
pub struct SuffixView {
    pub venue: String,
    /// None — сеть по умолчанию: ключ = id DEX как есть; Some(«@сеть») — ключ = id + суффикс.
    pub suffix: Option<String>,
}

/// Что делает: id площадок DEX шлюза (по алфавиту). Принимает: состояние API. Возвращает: список.
/// Ошибок нет: площадка, пропавшая из реестра между запросами, пропускается.
pub async fn dex_ids(st: &AppState) -> Vec<String> {
    let mut ids = st.registry.ids().await;
    ids.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = Vec::new();
    for id in ids {
        if let Ok(v) = st.registry.get(&id).await
            && v.kind() == VenueKind::Dex
        {
            out.push(id.0);
        }
    }
    out
}

/// Что делает: суффикс ключа области «DEX» площадки DEX (settings/dex_key.rs venue_suffix).
/// Принимает: id площадки. Возвращает: {venue, suffix}.
/// Ошибки: 404 — площадка не подключена; 400 — не DEX или суффикс не определить (причина текстом).
pub async fn suffix(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuffixView>, ApiError> {
    let venue = st
        .registry
        .get(&VenueId(id.clone()))
        .await
        .map_err(ApiError::gateway)?;
    if venue.kind() != VenueKind::Dex {
        return Err(ApiError::bad(format!(
            "{id}: не площадка DEX — ключей области «DEX» у неё нет"
        )));
    }
    let alone = dex_ids(&st).await.len() <= 1;
    let suffix = venue_suffix(&st.settings, &id, alone)
        .map_err(|e| ApiError::bad(format!("{id}: {}", e.message)))?;
    Ok(Json(SuffixView { venue: id, suffix }))
}
