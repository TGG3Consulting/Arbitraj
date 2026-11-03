//! Файл: api/wallets.rs
//! Что делает: экран «Маппинг кошельков» (решение 132) — список кошельков DEX и строк маппинга, добавить кошелёк (имя +
//! ключ: ключ сразу уходит в зашифрованный файл секретов, в SQLite — только имя секрета; ключ того же адреса, что у
//! другого кошелька, — отказ), переименовать, удалить (запрещено, пока в маппинге или в незавершённой сделке; секрет
//! ключа не удаляется), строка маппинга → кошелёк. Ответы НИКОГДА не содержат ключ: только имя секрета и адрес.
//!
//! GET    /api/wallets                    → {unlocked, dex, wallets[], rows[], shared[]}
//! POST   /api/wallets {name, key, password} → кошелёк (без ключа)
//! PATCH  /api/wallets/{id} {name}        → кошелёк
//! DELETE /api/wallets/{id}               → {removed}
//! PUT    /api/wallets/map {row, wallet}  → {changed} (wallet null — отвязать)
//! GET    /api/wallets/history?limit=N    → история «было/стало»
//!
//! Кто вызывает: api/server.rs; интерфейс ui/src/pages/Wallets.svelte.
//! От чего зависит: api/{state,secrets,wallets_dex,wallets_view}.rs, settings/accounts/, core/dex_account.rs, notify.
//! Настройки: security.kdf_* (запись секрета).

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::settings::accounts::edit_map::secret_name;

use super::state::{ApiError, AppState};
use super::wallets_dex::{dex_for, dex_id};

/// Кто меняет — для истории.
const BY: &str = "интерфейс";

/// Список для экрана. Ошибок нет (DEX не подключена — пустой dex, адресов нет).
pub async fn list(State(st): State<AppState>) -> Json<Value> {
    Json(super::wallets_view::view(&st).await)
}

/// Тело добавления.
#[derive(Deserialize)]
pub struct AddBody {
    pub name: String,
    pub key: Zeroizing<String>,
    pub password: Zeroizing<String>,
}

/// Добавляет кошелёк. Ошибки: 400 — секреты заблокированы, пустое или занятое имя, ключ не 64 шестнадцатеричных знака, неверный пароль,
/// ключ того же адреса, что у другого кошелька; 404 — DEX не подключена.
pub async fn add(
    State(st): State<AppState>,
    Json(b): Json<AddBody>,
) -> Result<Json<Value>, ApiError> {
    let hex = b.key.trim().trim_start_matches("0x");
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ApiError::bad(
            "Ключ: 64 шестнадцатеричных знака (можно с 0x)",
        ));
    }
    // Проверка «тот же адрес, что у другого кошелька» выводит адреса из ключей — нужна разблокировка.
    if !st.vault.status().unlocked {
        return Err(ApiError::bad(
            "Секреты заблокированы — сначала разблокируйте (экран «Разблокировка»)",
        ));
    }
    let a = st.settings.accounts();
    a.check_name(&b.name, None)
        .map_err(|e| ApiError::bad(e.to_string()))?;
    let dex = dex_id(&st).await?;
    let p = super::secrets::kdf(&st)?;
    let id = a
        .reserve_id()
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    let secret = secret_name(&id);
    st.vault
        .change(b.password.clone(), &secret, Some(b.key), p)
        .await
        .map_err(ApiError::bad)?;
    let queue = format!("{}:{id}", dex.0);
    let w = a
        .add(&id, &b.name, &secret, &queue, BY)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    // Тот же адрес, что у другого кошелька, — деньги смешались бы молча: запись и секрет убираются.
    let mine = dex_for(&st, &id).await?.wallet_address().await.ok();
    for o in a.wallets().into_iter().filter(|o| o.id != id) {
        let other = match dex_for(&st, &o.id).await {
            Ok(v) => v.wallet_address().await.ok(),
            Err(_) => None,
        };
        if mine.is_some() && other == mine {
            let _ = a.remove(&id, &[], "интерфейс (тот же адрес)").await;
            let _ = st.vault.change(b.password, &secret, None, p).await;
            return Err(ApiError::bad(format!(
                "Этот ключ — того же адреса, что у кошелька «{}»: кошелёк не добавлен",
                o.name
            )));
        }
    }
    let text = format!(
        "Кошелёк DEX «{}» добавлен (адрес {})",
        w.name,
        mine.unwrap_or_default()
    );
    st.notifier.send("wallet.added", None, &text).await;
    Ok(Json(json!(w)))
}

/// Тело переименования.
#[derive(Deserialize)]
pub struct RenameBody {
    pub name: String,
}

/// Переименовывает. Ошибки: 400 — нет кошелька, пустое или занятое имя.
pub async fn rename(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<RenameBody>,
) -> Result<Json<Value>, ApiError> {
    let w = st
        .settings
        .accounts()
        .rename(&id, &b.name, BY)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(Json(json!(w)))
}

/// Удаляет запись кошелька. Ошибки: 400 — нет кошелька, в маппинге, в незавершённой сделке.
pub async fn remove(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let busy = crate::core::dex_account::open_deals_of(&st.core, &id);
    let w = st
        .settings
        .accounts()
        .remove(&id, &busy, BY)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(Json(json!({ "removed": w })))
}

/// Тело строки маппинга.
#[derive(Deserialize)]
pub struct MapBody {
    pub row: String,
    pub wallet: Option<String>,
}

/// Привязывает строку маппинга к кошельку. Ошибки: 400 — неизвестная строка (не биржа реестра, не «DEX ↔ DEX», не
/// «пробы»), нет кошелька.
pub async fn map(
    State(st): State<AppState>,
    Json(b): Json<MapBody>,
) -> Result<Json<Value>, ApiError> {
    let rows = super::wallets_view::rows(&st).await;
    if !rows.iter().any(|(r, _)| r == &b.row) {
        return Err(ApiError::bad(format!("Строки маппинга «{}» нет", b.row)));
    }
    let a = st.settings.accounts();
    let changed = a
        .map_set(&b.row, b.wallet.as_deref(), BY)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    if changed {
        let name = b
            .wallet
            .as_deref()
            .and_then(|w| a.wallet(w))
            .map(|w| format!("«{}»", w.name))
            .unwrap_or_else(|| "нет кошелька".into());
        let text = format!("Маппинг кошельков: {} → {name}", b.row);
        st.notifier
            .send("wallet.mapping_changed", None, &text)
            .await;
    }
    Ok(Json(json!({ "changed": changed })))
}

/// Сколько записей истории читать.
#[derive(Deserialize)]
pub struct HistoryQuery {
    pub limit: u32,
}

/// История изменений кошельков и маппинга. Ошибки: 500 — база.
pub async fn history(
    State(st): State<AppState>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    st.settings
        .accounts()
        .history(q.limit)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}
