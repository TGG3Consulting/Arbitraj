//! Файл: notify/client.rs
//! Что делает: собирает клиента Telegram роли чата (решение 176, notify/roles.rs) из актуальных настроек и секретов
//! (адрес API, таймаут, токен роли, ID чата роли) — при каждом использовании, поэтому изменения действуют сразу.
//! Кто вызывает: notify/dispatch*.rs, notify/check.rs, notify/mirror*.rs, notify/updates*.rs, notify/ctl/*.
//! От чего зависит: settings, secrets, notify/telegram.rs, notify/roles.rs.
//! Настройки: notify.telegram_api_url, http.timeout_ms, ID чата роли (notify.telegram_chat у «Общего»,
//! notify.control_chat, notify.deals_chat, notify.failures_chat, notify.accounts_chat); секрет токена роли
//! (Role::token_secret).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use crate::gateway::errors::GatewayError;
use crate::secrets::Vault;
use crate::settings::read::At;
use crate::settings::store::Store;

use super::roles::Role;
use super::telegram::Telegram;

/// Клиент и чат, готовые к отправке.
pub struct Ready {
    pub tg: Telegram,
    pub chat: String,
    pub role: Role,
}

/// Собирает клиента «Общего» чата (как до решения 176).
/// Принимает: HTTP-клиент, хранилище настроек, секреты.
/// Возвращает: Ready. Ошибки: NotConfigured (не задана настройка или токен), SecretsLocked.
pub fn build(
    http: &reqwest::Client,
    settings: &Store,
    vault: &Vault,
) -> Result<Ready, GatewayError> {
    build_role(http, settings, vault, Role::General)
}

/// Собирает клиента роли. Принимает: HTTP-клиент, настройки, секреты, роль.
/// Возвращает: Ready. Ошибки: NotConfigured (не задан ID чата роли, адрес API, таймаут или токен), SecretsLocked.
pub fn build_role(
    http: &reqwest::Client,
    settings: &Store,
    vault: &Vault,
    role: Role,
) -> Result<Ready, GatewayError> {
    let url = settings.text("notify.telegram_api_url", At::default())?;
    let timeout = settings.integer("http.timeout_ms", At::default())?;
    let chat = settings.text(role.chat_key(), At::default())?;
    let token = vault.get(role.token_secret())?;
    Ok(Ready {
        tg: Telegram::new(http.clone(), &url, token, Duration::from_millis(timeout)),
        chat: chat.trim().to_string(),
        role,
    })
}

/// Отпечаток токена роли — чтобы роли с одним ботом делили ОДИН длинный опрос getUpdates (два опроса на один токен
/// Telegram не допускает: второй получает 409 Conflict). Сам токен наружу не уходит.
/// Принимает: секреты, роль. Возвращает: отпечаток или None — токена нет (секреты закрыты или не введён). Ошибок нет.
pub fn token_mark(vault: &Vault, role: Role) -> Option<u64> {
    let t = vault.get(role.token_secret()).ok()?;
    let mut h = DefaultHasher::new();
    t.as_str().hash(&mut h);
    Some(h.finish())
}
