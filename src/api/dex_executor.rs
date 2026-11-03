//! Файл: api/dex_executor.rs
//! Что делает: развёртывание своего исполнителя ArbExecutor (решение 122, часть X) для интерфейса — кнопка «Развернуть
//! исполнитель» (Площадки → DEX): оценка (газ, цена, стоимость, будущий адрес, хеш байткода) и отправка транзакции
//! создания по подтверждению человека. Действие с деньгами: отправка — только POST с SHA-256 байткода из оценки,
//! которую человек видел и подтвердил в окне. Сборка и отправка — у адаптера; api только находит площадку.
//! Кто вызывает: интерфейс (components/ExecutorDeploy.svelte).
//! От чего зависит: api/state.rs, gateway/registry.rs, gateway/contract.rs, core/types (executor.rs).
//! Настройки: нет (адрес исполнителя после развёртывания Тигран вписывает в exec.executor_address сам).
//!
//! GET  /api/venues/{id}/executor/quote?wallet=W  → ExecutorDeployQuote (ничего не отправляет); биржа — 400
//! POST /api/venues/{id}/executor/deploy?wallet=W {sha256} → ExecutorDeployed; хеш не тот — 502 (QuoteMismatch)
//! Решение 132: исполнитель — у кошелька W (его ключом, его очередью); код в сети совпал — адрес записывается у
//! кошелька (settings/accounts/) и уходит уведомление wallet.executor_deployed. Без wallet — отказ «кошелёк не выбран».

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;

use crate::core::types::{ExecutorDeployQuote, ExecutorDeployed};

use super::state::{ApiError, AppState};

/// Кошелёк исполнителя (решение 132).
#[derive(Deserialize)]
pub struct WalletQuery {
    pub wallet: Option<String>,
}

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
    Query(w): Query<WalletQuery>,
) -> Result<Json<ExecutorDeployQuote>, ApiError> {
    let venue = super::wallets_dex::venue_as(&st, &id, w.wallet.as_deref())
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
    Query(w): Query<WalletQuery>,
    Json(b): Json<DeployReq>,
) -> Result<Json<ExecutorDeployed>, ApiError> {
    if b.sha256.trim().is_empty() {
        return Err(ApiError::bad("нет sha256 подтверждённого байткода"));
    }
    let venue = super::wallets_dex::venue_as(&st, &id, w.wallet.as_deref())
        .await
        .map_err(ApiError::gateway)?;
    let d = venue
        .dex_executor_deploy(b.sha256.trim().to_string())
        .await
        .map_err(ApiError::gateway)?;
    // Решение 132: код в сети совпал — адрес у кошелька (владелец — он: развёрнуто его ключом).
    if let (true, Some(wid)) = (d.code_ok, w.wallet.as_deref()) {
        let a = st.settings.accounts();
        let addr = format!("{:#x}", d.address);
        if let Err(e) = a
            .set_executor(wid, Some(addr.clone()), "развёртывание")
            .await
        {
            return Err(ApiError::bad(format!(
                "Исполнитель {addr} развёрнут, но не записан у кошелька: {e}"
            )));
        }
        let name = a.wallet(wid).map(|x| x.name).unwrap_or_default();
        let text = format!(
            "Исполнитель кошелька «{name}» развёрнут: {addr} (tx {})",
            d.chain.hash
        );
        st.notifier
            .send("wallet.executor_deployed", None, &text)
            .await;
    }
    Ok(Json(d))
}
