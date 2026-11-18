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
//! POST /api/venues/{id}/executor/deploy?wallet=W {sha256, wnative} → ExecutorDeployed; хеш или обёртка не те — 502
//! (QuoteMismatch). Задача t114 (3в): wnative — адрес обёртки из оценки (аргумент конструктора, навсегда в исполнителе);
//! нет или не адрес — 400.
//! Решение 132: исполнитель — у кошелька W (его ключом, его очередью); код в сети совпал — адрес записывается у
//! кошелька (settings/accounts/) и уходит уведомление wallet.executor_deployed. Без wallet — отказ «кошелёк не выбран».
//! Решение 272: после записи адреса — сверка кода (check_executor): версия исполнителя записывается у кошелька.
//! Решение 408 (t193): развёртывание — core/executor_deploy.rs (общий путь с кнопкой Telegram).

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
    /// Задача t114 (3в): адрес обёртки из той же оценки (ExecutorDeployQuote.wnative). Option — чтобы отсутствие дало
    /// понятный 400, а не отказ разбора тела.
    pub wnative: Option<String>,
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

/// Развёртывание (шлюз `dex_executor_deploy`). Ошибки: 400 — пустой sha256; нет wnative или не адрес; не DEX
/// (NotApplicable); исполнитель развёрнут, но адрес не записан у кошелька; 404 — площадка не подключена
/// (VenueNotConnected); 409 — NonceConflict (ApiError::gateway; код ставит разбор ошибки узла при отправке —
/// exec/txs.rs send → venues/bsc_dex/errors.rs from_rpc); 502 — хеш или обёртка не те (QuoteMismatch), NotConfigured
/// «нет такого кошелька» (gateway/registry.rs get_as → for_account — до отправки, не сбой отправки) и прочие коды шлюза.
pub async fn deploy(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(w): Query<WalletQuery>,
    Json(b): Json<DeployReq>,
) -> Result<Json<ExecutorDeployed>, ApiError> {
    // Решение 408: один путь с кнопкой Telegram (core/executor_deploy.rs) — логика прежняя.
    use crate::core::executor_deploy::{DeployError, deploy as run};
    let reg = &st.registry;
    run(
        reg,
        &st.settings,
        &st.notifier,
        &id,
        w.wallet.as_deref(),
        &b.sha256,
        b.wnative.as_deref(),
    )
    .await
    .map(Json)
    .map_err(|e| match e {
        DeployError::Gateway(g) => ApiError::gateway(g),
        DeployError::Bad(t) => ApiError::bad(t),
    })
}
