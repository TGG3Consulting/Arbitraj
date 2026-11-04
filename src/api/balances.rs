//! Файл: api/balances.rs
//! Что делает: панель «Балансы» (решение 144, Тигран 04.11): балансы всех счетов каждой биржи (спот, фьючерсы, фонд,
//! earn… — контракт account_balances) по запросу интерфейса. Только чтение, на торговлю не влияет. Кошельки DEX
//! панель берёт существующим GET /api/wallets/{id}/balances (wallets_dex.rs).
//!
//! GET /api/balances          → {refresh_ms, refresh_error?, venues:[id биржи…]}
//! GET /api/balances/{venue}  → {venue, at, accounts:[{account, balances:[{asset, free, locked}], error?, note?}], error?}
//!
//! Автообновление ведёт интерфейс: пока панель открыта — запрос каждой биржи раз в ui.balances_refresh_ms (следующий —
//! только после ответа предыдущего, без наложения); кнопка ⟳ у биржи — запрос сразу, вне очереди.
//! Кто вызывает: api/server.rs; интерфейс ui/src/components/BalancesPanel.svelte.
//! От чего зависит: api/state.rs, gateway (реестр, контракт account_balances), settings/read.rs.
//! Настройки: ui.balances_refresh_ms (не задана — автообновления нет, только кнопки ⟳; причина видна в панели).

use axum::Json;
use axum::extract::{Path, State};
use chrono::Utc;
use serde_json::{Value, json};

use crate::core::types::VenueId;
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::state::{ApiError, AppState};

/// Список бирж для панели и интервал автообновления. Ошибок нет: нет настройки — refresh_error с причиной.
pub async fn list(State(st): State<AppState>) -> Json<Value> {
    let mut venues = Vec::new();
    for id in st.registry.ids().await {
        if let Ok(v) = st.registry.get(&id).await
            && v.kind() == VenueKind::Cex
        {
            venues.push(id.0);
        }
    }
    venues.sort();
    let at = At {
        venue: None,
        coin: None,
    };
    let (ms, err) = match st.settings.integer("ui.balances_refresh_ms", at) {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e.message)),
    };
    Json(json!({ "refresh_ms": ms, "refresh_error": err, "venues": venues }))
}

/// Балансы всех счетов одной биржи — свежим запросом к бирже. Ошибки: 404 — нет такой площадки, 400 — это не биржа;
/// сбой чтения — в поле error (панель показывает причину и прежние значения).
pub async fn venue(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let v = st
        .registry
        .get(&VenueId(id.clone()))
        .await
        .map_err(ApiError::gateway)?;
    if v.kind() != VenueKind::Cex {
        return Err(ApiError::bad(format!(
            "{id} — не биржа; балансы кошельков DEX — /api/wallets/{{id}}/balances"
        )));
    }
    let res = v.account_balances().await;
    let at = Utc::now();
    Ok(Json(match res {
        Ok(accounts) => json!({ "venue": id, "at": at, "accounts": accounts, "error": null }),
        Err(e) => json!({ "venue": id, "at": at, "accounts": [], "error": e.message }),
    }))
}
