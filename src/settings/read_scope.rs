//! Файл: settings/read_scope.rs
//! Что делает: типизированное чтение настроек ровно в одной области (DEX/вид пула, канал отправки,
//! событие) и чтение адресов сети BSC. Не задано — явная ошибка NotConfigured с именем и областью.
//! Кто вызывает: адаптер DEX (настройки dex.* и channel.*), позже ядро.
//! От чего зависит: settings/store.rs, settings/scope.rs, settings/read.rs (At), gateway/errors.rs.
//! Настройки: читает любые.

use std::str::FromStr;

use alloy_primitives::Address;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::read::At;
use super::scope::Scope;
use super::store::Store;

/// Ошибка «не задано в области». Принимает: имя, область.
fn not_set(name: &str, scope: &Scope) -> GatewayError {
    GatewayError::new(
        ErrorCode::NotConfigured,
        None,
        format!(
            "Не задана настройка {name} [{}] — задайте её в интерфейсе",
            scope.to_key()
        ),
    )
}

/// Ошибка «хранится в неверном виде». Принимает: имя.
fn bad(name: &str) -> GatewayError {
    GatewayError::new(
        ErrorCode::Internal,
        None,
        format!("Настройка {name} хранится в неверном виде"),
    )
}

impl Store {
    /// Значение ровно в области. Ошибка NotConfigured — не задано.
    fn exact(&self, name: &str, scope: &Scope) -> Result<Value, GatewayError> {
        self.get_exact(name, scope)
            .ok_or_else(|| not_set(name, scope))
    }

    /// Десятичное число в области. Ошибки: NotConfigured, Internal.
    pub fn decimal_in(&self, name: &str, scope: &Scope) -> Result<Decimal, GatewayError> {
        let v = self.exact(name, scope)?;
        v.as_str()
            .and_then(|s| Decimal::from_str(s).ok())
            .ok_or_else(|| bad(name))
    }

    /// Целое число в области. Ошибки: NotConfigured, Internal.
    pub fn integer_in(&self, name: &str, scope: &Scope) -> Result<u64, GatewayError> {
        self.exact(name, scope)?.as_u64().ok_or_else(|| bad(name))
    }

    /// Да/нет в области. Ошибки: NotConfigured, Internal.
    pub fn flag_in(&self, name: &str, scope: &Scope) -> Result<bool, GatewayError> {
        self.exact(name, scope)?.as_bool().ok_or_else(|| bad(name))
    }

    /// Текст, адрес URL или вариант выбора в области. Ошибки: NotConfigured, Internal.
    pub fn text_in(&self, name: &str, scope: &Scope) -> Result<String, GatewayError> {
        let v = self.exact(name, scope)?;
        v.as_str().map(str::to_string).ok_or_else(|| bad(name))
    }

    /// Адрес сети BSC в области. Ошибки: NotConfigured, Internal.
    pub fn address_in(&self, name: &str, scope: &Scope) -> Result<Address, GatewayError> {
        let v = self.exact(name, scope)?;
        v.as_str()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| bad(name))
    }

    /// Адрес сети BSC с подстановкой областей (монета → площадка → система).
    /// Ошибки: NotConfigured, Internal.
    pub fn address(&self, name: &str, at: At) -> Result<Address, GatewayError> {
        self.text(name, at)?.parse().map_err(|_| bad(name))
    }
}
