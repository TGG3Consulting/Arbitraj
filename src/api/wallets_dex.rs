//! Файл: api/wallets_dex.rs
//! Что делает: кошелёк DEX через шлюз (решение 132): площадка DEX на счёт кошелька, адрес кошелька (выводит адаптер из
//! ключа; ключ наружу не отдаётся), балансы BNB и USDT, «Проверить подключение» кошелька, ручная запись адреса
//! исполнителя (только после проверки «код совпал, владелец — этот кошелёк»).
//!
//! GET  /api/wallets/{id}/balances?venue=           → {bnb, usdt, quote, error?}
//! POST /api/check/wallet/{id}?venue=                → ConnectionCheck («Подключено: «имя»: адрес …, BNB …» или причина)
//! PUT  /api/wallets/{id}/executor?venue= {address}  → кошелёк (address null — снять); адрес проверяется на узле
//! Решение 432: `venue` — площадка DEX сети (api/wallets_chain.rs); не указан — DEX сети по умолчанию, как раньше;
//! исполнитель — у кошелька на этой площадке (settings/accounts/chain_exec.rs).
//!
//! Кто вызывает: api/server.rs, api/wallets.rs (адреса в списке), api/dex_executor.rs, api/wallet_queue.rs, api/check.rs.
//! От чего зависит: api/state.rs, gateway (реестр, контракт), settings/accounts/.
//! Настройки: venue.quote_asset [площадка DEX] (тикер USDT).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::core::types::{DexAccountId, VenueId};
use crate::gateway::contract::{ConnectionCheck, Venue};
use crate::gateway::errors::GatewayResult;
use crate::settings::read::At;

use super::state::{ApiError, AppState};
use super::wallets_chain::VenueQuery;

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

/// Площадка DEX сети по умолчанию (решение 430: вместо «первой DEX реестра» — core/chain.rs dex_of; одна DEX — она).
/// Выбор площадки DEX по сети в интерфейсе — шаг 4 плана двух сетей. Ошибки: 404 — DEX не подключена или сеть по
/// умолчанию не выбрать (причина).
pub async fn dex_id(st: &AppState) -> Result<VenueId, ApiError> {
    let dex = super::wallets_chain::dex_ids(st).await;
    if dex.is_empty() {
        return Err(ApiError::no_data("площадка DEX не подключена"));
    }
    crate::core::chain::dex_of(&st.settings, &dex, None).map_err(ApiError::no_data)
}

/// DEX на кошелёк. Ошибки: 404 — нет DEX; 400 — нет такого кошелька.
pub async fn dex_for(st: &AppState, wallet: &str) -> Result<Arc<dyn Venue>, ApiError> {
    dex_for_at(st, wallet, None).await
}

/// Решение 432: DEX сети `venue` на кошелёк (None — DEX сети по умолчанию, как dex_for). Ошибки: 400 — нет кошелька или
/// не площадка DEX; 404 — нет DEX.
pub async fn dex_for_at(
    st: &AppState,
    wallet: &str,
    venue: Option<&str>,
) -> Result<Arc<dyn Venue>, ApiError> {
    if st.settings.accounts().wallet(wallet).is_none() {
        return Err(ApiError::bad(format!("Кошелька {wallet} нет")));
    }
    let id = super::wallets_chain::pick(st, venue).await?;
    venue_as(st, &id.0, Some(wallet))
        .await
        .map_err(ApiError::gateway)
}

/// Балансы BNB и USDT кошелька. Ошибки: 400/404 — как у dex_for; сбой запроса — в поле error.
pub async fn balances(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<VenueQuery>,
) -> Result<Json<Value>, ApiError> {
    let v = dex_for_at(&st, &id, q.venue.as_deref()).await?;
    let dex = super::wallets_chain::pick(&st, q.venue.as_deref()).await?;
    let at = At {
        venue: Some(&dex.0),
        coin: None,
    };
    let quote = st.settings.text("venue.quote_asset", at).ok();
    let one = |a: String| {
        let v = v.clone();
        async move { v.balance(&a).await.map(|b| b.map(|x| x.free)) }
    };
    // Решение 416: тикер газа — dex.gas_ticker площадки DEX; не задана — причина в поле error.
    let bnb = match crate::core::gas_ticker::at(&st.settings, &dex) {
        Ok(g) => one(g).await,
        Err(e) => Err(crate::gateway::errors::GatewayError::not_set(
            None,
            "dex.gas_ticker",
            e,
        )),
    };
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
pub async fn check(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<VenueQuery>,
) -> Json<ConnectionCheck> {
    let res = match dex_for_at(&st, &id, q.venue.as_deref()).await {
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
    Query(q): Query<VenueQuery>,
    Json(b): Json<ExecutorBody>,
) -> Result<Json<Value>, ApiError> {
    let a = st.settings.accounts();
    // Решение 432: исполнитель кошелька на площадке DEX сети запроса (домашняя площадка — прежние поля).
    let dex = super::wallets_chain::pick(&st, q.venue.as_deref()).await?;
    let at = dex.0.as_str();
    let before = a
        .wallet_at(at, &id)
        .ok_or_else(|| ApiError::bad(format!("Кошелька {id} нет")))?;
    let addr = b
        .address
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty());
    if let Some(x) = &addr {
        x.parse::<alloy_primitives::Address>()
            .map_err(|_| ApiError::bad(format!("{x} — не адрес")))?;
    }
    a.set_executor_at(at, &id, addr.clone(), "интерфейс")
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    if addr.is_some() {
        let v = dex_for_at(&st, &id, Some(at)).await?;
        let c = v.check_executor().await.map_err(ApiError::gateway)?;
        if !c.ok {
            // Проверка не прошла — прежний адрес обратно. Ревью 272 (Б2): и прежняя сверенная версия с обёрткой — смена
            // адреса их сбросила, а память сверки (onchain.rs) осталась при старом адресе: без восстановления нативный
            // BNB через исполнитель молча выключился бы до перезапуска.
            let _ = a
                .set_executor_at(at, &id, before.executor.clone(), "интерфейс (откат)")
                .await;
            if let Some(old) = before.executor.as_deref() {
                let _ = a
                    .set_executor_version_at(
                        at,
                        &id,
                        old,
                        before.executor_version.clone(),
                        before.executor_wrapped.clone(),
                        "интерфейс (откат)",
                    )
                    .await;
            }
            return Err(ApiError::bad(format!(
                "Исполнитель не записан: {}",
                c.details
            )));
        }
    }
    Ok(Json(json!(a.wallet_at(at, &id))))
}
