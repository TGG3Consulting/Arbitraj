//! Файл: core/executor_deploy.rs
//! Что делает: решение 408 (t193) — развёртывание исполнителя ArbExecutor кошелька по подтверждению человека, одним
//! путём для интерфейса (api/dex_executor.rs deploy, кнопка «Развернуть исполнитель» окна ExecutorDeploy.svelte) и
//! Telegram (кнопка тревоги wallet.executor_v1 → «Да» → команда ядра ManualAction::DeployExecutor). Логика перенесена
//! из api/dex_executor.rs без изменений: отправка — у адаптера (контракт dex_executor_deploy, sha256 и обёртка из
//! оценки, которую человек видел); код в сети совпал — адрес записывается у кошелька (решение 132), сверка кода пишет
//! версию (решение 272), уведомление wallet.executor_deployed. Бот сам не разворачивает: вызывают только кнопки человека.
//! Кто вызывает: api/dex_executor.rs (deploy), core/commands_ext.rs (DeployExecutor).
//! От чего зависит: gateway (Registry::get_as, контракт Venue), settings/accounts (set_executor), notify (Notifier).
//! Настройки: нет.

use alloy_primitives::Address;

use crate::core::types::{DexAccountId, ExecutorDeployed, VenueId};
use crate::gateway::errors::GatewayError;
use crate::gateway::registry::Registry;
use crate::notify::dispatch::Notifier;
use crate::settings::store::Store;

/// Сбой развёртывания: ошибка шлюза (код — для ответа интерфейсу) или отказ до / после отправки текстом.
#[derive(Debug)]
pub enum DeployError {
    Gateway(GatewayError),
    Bad(String),
}

impl std::fmt::Display for DeployError {
    /// Текст для человека. Ошибок нет.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gateway(e) => write!(f, "{}", e.message),
            Self::Bad(t) => write!(f, "{t}"),
        }
    }
}

/// Развернуть исполнитель кошелька. Принимает: реестр шлюза, настройки, уведомления, площадку DEX, id кошелька,
/// SHA-256 байткода из оценки, адрес обёртки из оценки (строка). Возвращает: итог развёртывания (текст — со сверкой).
/// Ошибки: Bad — пустой sha256, обёртка не адрес, адрес не записан у кошелька; Gateway — ошибки шлюза (площадка не
/// подключена, NonceConflict, QuoteMismatch и др.).
pub async fn deploy(
    gw: &Registry,
    settings: &Store,
    notifier: &Notifier,
    venue: &str,
    wallet: Option<&str>,
    sha256: &str,
    wnative: Option<&str>,
) -> Result<ExecutorDeployed, DeployError> {
    if sha256.trim().is_empty() {
        return Err(DeployError::Bad(
            "нет sha256 подтверждённого байткода".into(),
        ));
    }
    let wnative: Address = wnative
        .map(str::trim)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            DeployError::Bad("нет адреса обёртки (wnative) из подтверждённой оценки".into())
        })?;
    let acc = wallet.map(|w| DexAccountId(w.to_string()));
    let v = gw
        .get_as(&VenueId(venue.to_string()), acc.as_ref())
        .await
        .map_err(DeployError::Gateway)?;
    let d = v
        .dex_executor_deploy(sha256.trim().to_string(), wnative)
        .await
        .map_err(DeployError::Gateway)?;
    // Решение 132: код в сети совпал — адрес у кошелька (владелец — он: развёрнуто его ключом).
    let (true, Some(wid)) = (d.code_ok, wallet) else {
        return Ok(d);
    };
    let a = settings.accounts();
    let addr = format!("{:#x}", d.address);
    if let Err(e) = a
        .set_executor(wid, Some(addr.clone()), "развёртывание")
        .await
    {
        return Err(DeployError::Bad(format!(
            "Исполнитель {addr} развёрнут, но не записан у кошелька: {e}"
        )));
    }
    // Решение 272: сверка кода по новому адресу — версия (v2) записывается у кошелька (onchain.rs), иначе нативный
    // BNB через исполнитель не пошёл бы до первой сверки. Сбой сверки — в ответ текстом, адрес уже записан.
    let checked = v
        .check_executor()
        .await
        .map(|c| c.details)
        .unwrap_or_else(|e| e.message);
    let name = a.wallet(wid).map(|x| x.name).unwrap_or_default();
    let text = format!(
        "Исполнитель кошелька «{name}» развёрнут: {addr} (tx {})",
        d.chain.hash
    );
    notifier.send("wallet.executor_deployed", None, &text).await;
    let mut d = d;
    d.text = format!("{} Сверка: {checked}", d.text);
    Ok(d)
}
