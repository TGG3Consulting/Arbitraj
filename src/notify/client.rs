//! Файл: notify/client.rs
//! Что делает: собирает клиента Telegram из актуальных настроек и секретов
//! (адрес API, таймаут, токен) — при каждом использовании, поэтому изменения действуют сразу.
//! Кто вызывает: notify/dispatch.rs, notify/check.rs.
//! От чего зависит: settings, secrets, notify/telegram.rs.
//! Настройки: notify.telegram_api_url, http.timeout_ms, notify.telegram_chat; секрет telegram.bot_token.

use std::time::Duration;

use crate::gateway::errors::GatewayError;
use crate::secrets::{TELEGRAM_BOT_TOKEN, Vault};
use crate::settings::read::At;
use crate::settings::store::Store;

use super::telegram::Telegram;

/// Клиент и чат, готовые к отправке.
pub struct Ready {
    pub tg: Telegram,
    pub chat: String,
}

/// Собирает клиента.
/// Принимает: HTTP-клиент, хранилище настроек, секреты.
/// Возвращает: Ready. Ошибки: NotConfigured (не задана настройка или токен), SecretsLocked.
pub fn build(
    http: &reqwest::Client,
    settings: &Store,
    vault: &Vault,
) -> Result<Ready, GatewayError> {
    let url = settings.text("notify.telegram_api_url", At::default())?;
    let timeout = settings.integer("http.timeout_ms", At::default())?;
    let chat = settings.text("notify.telegram_chat", At::default())?;
    let token = vault.get(TELEGRAM_BOT_TOKEN)?;
    Ok(Ready {
        tg: Telegram::new(http.clone(), &url, token, Duration::from_millis(timeout)),
        chat,
    })
}
