//! Файл: api/balances.rs
//! Что делает: панель «Балансы» (решение 144, Тигран 04.11): балансы всех счетов каждой биржи (спот, фьючерсы, фонд,
//! earn… — контракт account_balances) по запросу интерфейса. Чтение балансов на торговлю не влияет; кнопка «Продать» (решение 174) — команда ядра. Кошельки DEX
//! панель берёт существующим GET /api/wallets/{id}/balances (wallets_dex.rs).
//!
//! GET /api/balances          → {refresh_ms, refresh_error?, venues:[id биржи…]}
//! GET /api/balances/{venue}  → {venue, at, accounts:[{account, balances:[{asset, free, locked, sell}], error?, note?}], error?}
//!   sell (решение 174) — кнопка «Продать» строки: null — кнопки нет (котируемая монета биржи); {ok:true} — доступна;
//!   {ok:false, why} — недоступна, причина по наведению (core/balance_sell_check.rs). Нажатие — POST /api/manual/command
//!   с {"SellBalance":{venue, account, asset}} (core/balance_sell.rs).
//!
//! Автообновление ведёт интерфейс: пока панель открыта — запрос каждой биржи раз в ui.balances_refresh_ms (следующий —
//! только после ответа предыдущего, без наложения); кнопка ⟳ у биржи — запрос сразу, вне очереди.
//! Кто вызывает: api/server.rs; интерфейс ui/src/components/BalancesPanel.svelte.
//! От чего зависит: api/state.rs, gateway (реестр, контракт account_balances), settings/read.rs,
//! core/balances_cache.rs (ответ и кнопка «Продать», решение 174; память для Telegram, решение 176).
//! Настройки: ui.balances_refresh_ms (не задана — автообновления нет, только кнопки ⟳; причина видна в панели).

use axum::Json;
use axum::extract::{Path, State};
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

/// Балансы всех счетов одной биржи — свежим запросом к бирже (ответ кладётся в память ядра — её читает Telegram,
/// решение 176, core/balances_cache.rs). Ошибки: 404 — нет такой площадки, 400 — это не биржа; сбой чтения — в поле
/// error (панель показывает причину и прежние значения).
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
    let res = st.core.venue_balances(&VenueId(id), None).await;
    res.map(|b| Json(b.to_json())).map_err(ApiError::bad)
}
