//! Файл: settings/read.rs
//! Что делает: типизированное чтение настроек для компонентов. Не задано — явная ошибка
//! «не задано» с именем настройки, функция, которой она нужна, не работает.
//! Кто вызывает: все компоненты (notify, позже ядро и адаптеры).
//! От чего зависит: settings/store.rs, gateway/errors.rs (код NotConfigured).
//! Настройки: читает любые.

use std::str::FromStr;

use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::store::Store;

/// Где искать значение: площадка и монета (если применимо).
#[derive(Debug, Clone, Copy, Default)]
pub struct At<'a> {
    pub venue: Option<&'a str>,
    pub coin: Option<&'a str>,
}

/// Ошибка «не задано» с понятным текстом и именем настройки в поле setting (GatewayError::not_set).
fn not_set(name: &str) -> GatewayError {
    GatewayError::not_set(
        None,
        name,
        format!("Не задана настройка {name} — задайте её в интерфейсе"),
    )
}

/// Ошибка «значение хранится в неверном виде» (повреждение базы). Принимает: имя настройки.
fn bad(name: &str) -> GatewayError {
    GatewayError::new(
        ErrorCode::Internal,
        None,
        format!("Настройка {name} хранится в неверном виде"),
    )
}

impl Store {
    /// Десятичное число. Ошибки: NotConfigured, Internal (неверный вид).
    pub fn decimal(&self, name: &str, at: At) -> Result<Decimal, GatewayError> {
        let v = self
            .resolve(name, at.venue, at.coin)
            .ok_or_else(|| not_set(name))?;
        v.as_str()
            .and_then(|s| Decimal::from_str(s).ok())
            .ok_or_else(|| bad(name))
    }

    /// Целое число. Ошибки: NotConfigured, Internal.
    pub fn integer(&self, name: &str, at: At) -> Result<u64, GatewayError> {
        let v = self
            .resolve(name, at.venue, at.coin)
            .ok_or_else(|| not_set(name))?;
        v.as_u64().ok_or_else(|| bad(name))
    }

    /// Да/нет. Ошибки: NotConfigured, Internal.
    pub fn flag(&self, name: &str, at: At) -> Result<bool, GatewayError> {
        let v = self
            .resolve(name, at.venue, at.coin)
            .ok_or_else(|| not_set(name))?;
        v.as_bool().ok_or_else(|| bad(name))
    }

    /// Текст или адрес. Ошибки: NotConfigured, Internal.
    pub fn text(&self, name: &str, at: At) -> Result<String, GatewayError> {
        let v = self
            .resolve(name, at.venue, at.coin)
            .ok_or_else(|| not_set(name))?;
        v.as_str().map(str::to_string).ok_or_else(|| bad(name))
    }

    /// Список строк. Ошибки: NotConfigured, Internal.
    pub fn list(&self, name: &str, at: At) -> Result<Vec<String>, GatewayError> {
        let v = self
            .resolve(name, at.venue, at.coin)
            .ok_or_else(|| not_set(name))?;
        v.as_array()
            .and_then(|a| a.iter().map(|x| x.as_str().map(str::to_string)).collect())
            .ok_or_else(|| bad(name))
    }

    /// Настройка события уведомлений (область «событие»). Ошибки: как у функций выше.
    pub fn event_value(&self, name: &str, event: &str) -> Option<serde_json::Value> {
        self.get_exact(name, &super::value::Scope::Event(event.to_string()))
    }
}
