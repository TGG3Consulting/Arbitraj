//! Файл: secrets/mod.rs
//! Что делает: доступ к секретам (ключи бирж, ключ кошелька, токен Telegram) — только в памяти.
//! Пока секреты не разблокированы паролем, любой запрос секрета возвращает SecretsLocked.
//! Кто вызывает: notify (токен Telegram), позже адаптеры (ключи), api (разблокировка).
//! От чего зависит: gateway/errors.rs.
//! Настройки: параметры шифрования — security.kdf_* (после утверждения алгоритма).
//!
//! СОСТОЯНИЕ: зашифрованный файл и разблокировка паролем ещё НЕ сделаны — ждут утверждения
//! алгоритма Тиграном (Issue #1, пункт 7). До этого хранилище всегда заблокировано,
//! и все функции, которым нужен секрет, честно сообщают «секреты не разблокированы».

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::gateway::errors::{ErrorCode, GatewayError};

/// Имя секрета токена Telegram-бота.
pub const TELEGRAM_BOT_TOKEN: &str = "telegram.bot_token";

/// Секреты в памяти. None — заблокировано.
#[derive(Clone, Default)]
pub struct Vault {
    keys: Arc<RwLock<Option<HashMap<String, String>>>>,
}

impl Vault {
    /// Создаёт заблокированное хранилище.
    pub fn locked() -> Self {
        Self::default()
    }

    /// Разблокировано ли хранилище. Ошибок нет.
    pub fn is_unlocked(&self) -> bool {
        self.keys.read().map(|k| k.is_some()).unwrap_or(false)
    }

    /// Возвращает секрет по имени.
    /// Ошибки: SecretsLocked — не разблокировано; NotConfigured — такой секрет не введён.
    pub fn get(&self, name: &str) -> Result<String, GatewayError> {
        let guard = self.keys.read().unwrap_or_else(|p| p.into_inner());
        let keys = guard.as_ref().ok_or_else(|| {
            GatewayError::new(
                ErrorCode::SecretsLocked,
                None,
                "Секреты не разблокированы — введите пароль в интерфейсе",
            )
        })?;
        keys.get(name).cloned().ok_or_else(|| {
            GatewayError::new(
                ErrorCode::NotConfigured,
                None,
                format!("Секрет {name} не введён — добавьте его в интерфейсе"),
            )
        })
    }
}
