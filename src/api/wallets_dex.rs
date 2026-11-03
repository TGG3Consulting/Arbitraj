//! Файл: api/wallets_dex.rs
//! Что делает: кошелёк DEX через шлюз (решение 132): площадка DEX на счёт кошелька, адрес кошелька (выводит адаптер из
//! ключа; ключ наружу не отдаётся), балансы BNB и USDT, «Проверить подключение» кошелька, ручная запись адреса
//! исполнителя (только после проверки «код совпал, владелец — этот кошелёк»).
//!
//! GET  /api/wallets/{id}/balances           → {bnb, usdt, quote, error?}
//! POST /api/check/wallet/{id}                → ConnectionCheck («Подключено: «имя»: адрес …, BNB …» или причина)
//! PUT  /api/wallets/{id}/executor {address}  → кошелёк (address null — снять); адрес проверяется на узле
//!
//! Кто вызывает: api/server.rs, api/wallets.rs (адреса в списке), api/dex_executor.rs, api/wallet_queue.rs, api/check.rs.
//! От чего зависит: api/state.rs, gateway (реестр, контракт), settings/accounts/.
//! Настройки: venue.quote_asset [площадка DEX] (тикер USDT).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::core::types::{DexAccountId, VenueId};
use crate::gateway::contract::{ConnectionCheck, Venue, VenueKind};
use crate::gateway::errors::GatewayResult;
use crate::settings::read::At;

use super::state::{ApiError, AppState};

/// Площадка по id, привязанная к кошельку (wallet — id кошелька; None — без привязки). Ошибки: шлюз.
pub async fn venue_as(
    st: &AppState,
    venue: &str,
    wallet: Option<&str>,
) -> GatewayResult<Arc<dyn Venue>> {
    let acc = wallet.map(|w| DexAccountId(w.to_string()));
    st.registry
        .get_as(&VenueId(venue.into()), acc.as_ref())
        .await
}

/// Первая площадка DEX реестра (кошельки — у неё). Ошибки: 404 — DEX не подключена.
pub async fn dex_id(st: &AppState) -> Result<VenueId, ApiError> {
    for id in st.registry.ids().await {
        if let Ok(v) = st.registry.get(&id).await
            && v.kind() == VenueKind::Dex
        {
            return Ok(id);
        }
    }
    Err(ApiError::no_data("площадка DEX не подключена"))
}

/// DEX на кошелёк. Ошибки: 404 — нет DEX; 400 — нет такого кошелька.
pub async fn dex_for(st: &AppState, wallet: &str) -> Result<Arc<dyn Venue>, ApiError> {
    if st.settings.accounts().wallet(wallet).is_none() {
        return Err(ApiError::bad(format!("Кошелька {wallet} нет")));
    }
    let id = dex_id(st).await?;
    venue_as(st, &id.0, Some(wallet))
        .await
        .map_err(ApiError::gateway)
}

/// Балансы BNB и USDT кошелька. Ошибки: 400/404 — как у dex_for; сбой запроса — в поле error.
pub async fn balances(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let v = dex_for(&st, &id).await?;
    let dex = dex_id(&st).await?;
    let at = At {
        venue: Some(&dex.0),
        coin: None,
    };
    let quote = st.settings.text("venue.quote_asset", at).ok();
    let one = |a: String| {
        let v = v.clone();
        async move { v.balance(&a).await.map(|b| b.map(|x| x.free)) }
    };
    let bnb = one(crate::core::convert::GAS_TICKER.to_string()).await;
    let usdt = match &quote {
        Some(q) => Some(one(q.clone()).await),
        None => None,
    };
    let err = [
        bnb.as_ref().err(),
        usdt.as_ref().and_then(|u| u.as_ref().err()),
    ]
    .into_iter()
    .flatten()
    .map(|e| e.message.clone())
    .collect::<Vec<_>>();
    Ok(Json(json!({
        "bnb": bnb.ok().flatten(),
        "usdt": usdt.and_then(|u| u.ok().flatten()),
        "quote": quote,
        "error": if err.is_empty() { None } else { Some(err.join("; ")) },
    })))
}

/// «Проверить подключение» кошелька: узел, адрес и BNB кошелька (шлюз check_connection у DEX на счёт).
/// Ошибок нет: причина — в details.
pub async fn check(State(st): State<AppState>, Path(id): Path<String>) -> Json<ConnectionCheck> {
    let res = match dex_for(&st, &id).await {
        Ok(v) => v
            .check_connection()
            .await
            .unwrap_or_else(|e| ConnectionCheck {
                ok: false,
                details: e.message,
            }),
        Err(e) => ConnectionCheck {
            ok: false,
            details: e.1,
        },
    };
    Json(res)
}

/// Тело ручной записи исполнителя.
#[derive(Deserialize)]
pub struct ExecutorBody {
    /// Адрес исполнителя (0x…); None — снять.
    pub address: Option<String>,
}

/// Записывает адрес исполнителя у кошелька: перед записью — проверка на узле (код совпал, владелец — этот кошелёк);
/// не прошла — не записывается. Снять (null) — без проверки. Ошибки: 400 — нет кошелька, проверка не прошла.
pub async fn set_executor(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<ExecutorBody>,
) -> Result<Json<Value>, ApiError> {
    let a = st.settings.accounts();
    let before = a
        .wallet(&id)
        .ok_or_else(|| ApiError::bad(format!("Кошелька {id} нет")))?;
    let addr = b
        .address
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty());
    if let Some(x) = &addr {
        x.parse::<alloy_primitives::Address>()
            .map_err(|_| ApiError::bad(format!("{x} — не адрес")))?;
    }
    a.set_executor(&id, addr.clone(), "интерфейс")
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    if addr.is_some() {
        let v = dex_for(&st, &id).await?;
        let c = v.check_executor().await.map_err(ApiError::gateway)?;
        if !c.ok {
            // Проверка не прошла — прежний адрес обратно.
            let _ = a
                .set_executor(&id, before.executor, "интерфейс (откат)")
                .await;
            return Err(ApiError::bad(format!(
                "Исполнитель не записан: {}",
                c.details
            )));
        }
    }
    Ok(Json(json!(a.wallet(&id))))
}
