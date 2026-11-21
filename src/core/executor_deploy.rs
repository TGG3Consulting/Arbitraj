//! Файл: core/executor_deploy.rs
//! Что делает: решение 408 (t193) — развёртывание исполнителя ArbExecutor кошелька по подтверждению человека, одним
//! путём для интерфейса (api/dex_executor.rs deploy, кнопка «Развернуть исполнитель» окна ExecutorDeploy.svelte) и
//! Telegram (кнопка тревоги wallet.executor_v1 → «Да» → команда ядра ManualAction::DeployExecutor). Логика перенесена
//! из api/dex_executor.rs без изменений: отправка — у адаптера (контракт dex_executor_deploy, sha256 и обёртка из
//! оценки, которую человек видел); код в сети совпал — адрес записывается у кошелька (решение 132), сверка кода пишет
//! версию (решение 272), уведомление wallet.executor_deployed. Бот сам не разворачивает: вызывают только кнопки человека.
//! Ревью t193 (Н1): перед отправкой — исполнитель кошелька тот же, что человек видел (и для кнопки тревоги — всё ещё v1);
//! (М3) от этой сверки до записи нового адреса — одно развёртывание на кошелёк (второе одновременное — отказ).
//! Кто вызывает: api/dex_executor.rs (deploy), core/commands_ext.rs (DeployExecutor).
//! От чего зависит: gateway (Registry::get_as, контракт Venue), settings/accounts (set_executor), notify (Notifier).
//! Решение 432: исполнитель — у кошелька на площадке DEX развёртывания (settings/accounts/chain_exec.rs set_executor_at;
//! домашняя площадка кошелька — прежние поля).
//! Настройки: нет.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

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

/// Ревью t193 (М3): кошельки, у которых сейчас идёт развёртывание (интерфейс и Telegram одновременно — второе отказ).
static DEPLOYING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

/// Отметка «развёртывание кошелька идёт»: снимается при выходе из deploy любым путём.
struct Busy(String);

impl Busy {
    /// Поставить отметку. Принимает: id кошелька. Ошибки: Bad — развёртывание этого кошелька уже идёт.
    fn take(wallet: &str) -> Result<Self, DeployError> {
        let fresh = DEPLOYING
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(wallet.to_string());
        if !fresh {
            return Err(DeployError::Bad(format!(
                "развёртывание исполнителя кошелька {wallet} уже идёт — дождитесь его итога"
            )));
        }
        Ok(Self(wallet.to_string()))
    }
}

impl Drop for Busy {
    /// Снять отметку. Ошибок нет.
    fn drop(&mut self) {
        DEPLOYING
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.0);
    }
}

/// Запрос развёртывания — то, что человек видел и подтвердил.
pub struct Req<'a> {
    /// Площадка DEX.
    pub venue: &'a str,
    /// id кошелька (решение 132); None — отказ шлюза «кошелёк не выбран».
    pub wallet: Option<&'a str>,
    /// SHA-256 байткода из оценки.
    pub sha256: &'a str,
    /// Адрес обёртки из оценки (строка 0x…).
    pub wnative: Option<&'a str>,
    /// Ревью t193 (Н1): адрес исполнителя кошелька, который человек видел (None — исполнителя не было).
    pub executor: Option<&'a str>,
    /// Ревью t193 (Н1): только переразвёртывание v1 (кнопка тревоги wallet.executor_v1).
    pub v1_only: bool,
}

/// Ревью t193 (Н1): исполнитель кошелька не сменился с вопроса (и, для кнопки тревоги, всё ещё v1) — иначе повторное
/// или устаревшее «Да» развернуло бы ещё раз. Принимает: настройки, запрос. Ошибки: Bad — текст отказа.
fn same(settings: &Store, r: &Req<'_>) -> Result<(), DeployError> {
    let Some(wid) = r.wallet else {
        return Ok(());
    };
    // Решение 432: исполнитель кошелька на площадке DEX развёртывания (её сети).
    let w = settings
        .accounts()
        .wallet_at(r.venue, wid)
        .ok_or_else(|| DeployError::Bad(format!("кошелька {wid} нет")))?;
    let low = |x: Option<&str>| x.map(|a| a.trim().to_ascii_lowercase());
    let (now, seen) = (low(w.executor.as_deref()), low(r.executor));
    if now != seen {
        return Err(DeployError::Bad(format!(
            "исполнитель кошелька «{}» уже сменился: в вопросе {}, сейчас {} — откройте оценку заново",
            w.name,
            seen.as_deref().unwrap_or("не было"),
            now.as_deref().unwrap_or("нет")
        )));
    }
    if r.v1_only && w.executor_version.as_deref() != Some("v1") {
        return Err(DeployError::Bad(format!(
            "исполнитель кошелька «{}» уже не v1 (версия {}) — переразвёртывание не нужно",
            w.name,
            w.executor_version.as_deref().unwrap_or("не сверена")
        )));
    }
    Ok(())
}

/// Развернуть исполнитель кошелька. Принимает: реестр шлюза, настройки, уведомления, запрос. Возвращает: итог
/// развёртывания (текст — со сверкой). Ошибки: Bad — пустой sha256, обёртка не адрес, исполнитель сменился с вопроса
/// (ревью t193 Н1), адрес не записан у кошелька; Gateway — ошибки шлюза (площадка не подключена, NonceConflict,
/// QuoteMismatch и др.).
pub async fn deploy(
    gw: &Registry,
    settings: &Store,
    notifier: &Notifier,
    r: &Req<'_>,
) -> Result<ExecutorDeployed, DeployError> {
    let (venue, wallet, sha256) = (r.venue, r.wallet, r.sha256);
    if sha256.trim().is_empty() {
        return Err(DeployError::Bad(
            "нет sha256 подтверждённого байткода".into(),
        ));
    }
    let wnative: Address = r
        .wnative
        .map(str::trim)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            DeployError::Bad("нет адреса обёртки (wnative) из подтверждённой оценки".into())
        })?;
    // Ревью t193 (М3): от сверки «исполнитель не сменился» до записи нового адреса — одно развёртывание на кошелёк.
    let _busy = wallet.map(Busy::take).transpose()?;
    same(settings, r)?;
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
        .set_executor_at(venue, wid, Some(addr.clone()), "развёртывание")
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
