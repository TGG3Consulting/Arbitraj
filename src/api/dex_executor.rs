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
    if b.sha256.trim().is_empty() {
        return Err(ApiError::bad("нет sha256 подтверждённого байткода"));
    }
    let wnative: alloy_primitives::Address = b
        .wnative
        .as_deref()
        .map(str::trim)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| ApiError::bad("нет адреса обёртки (wnative) из подтверждённой оценки"))?;
    let venue = super::wallets_dex::venue_as(&st, &id, w.wallet.as_deref())
        .await
        .map_err(ApiError::gateway)?;
    let d = venue
        .dex_executor_deploy(b.sha256.trim().to_string(), wnative)
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
        // Решение 272: сверка кода по новому адресу — версия (v2) записывается у кошелька (onchain.rs), иначе нативный
        // BNB через исполнитель не пошёл бы до первой сверки. Сбой сверки — в ответ текстом, адрес уже записан.
        let checked = venue
            .check_executor()
            .await
            .map(|c| c.details)
            .unwrap_or_else(|e| e.message);
        let name = a.wallet(wid).map(|x| x.name).unwrap_or_default();
        let text = format!(
            "Исполнитель кошелька «{name}» развёрнут: {addr} (tx {})",
            d.chain.hash
        );
        st.notifier
            .send("wallet.executor_deployed", None, &text)
            .await;
        let mut d = d;
        d.text = format!("{} Сверка: {checked}", d.text);
        return Ok(Json(d));
    }
    Ok(Json(d))
}
