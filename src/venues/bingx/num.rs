//! Файл: venues/bingx/num.rs
//! Что делает: чтение чисел и полей из ответов BingX без потери точности.
//! BingX присылает числа то строкой ("0.0006"), то числом (0.01, 1e-05) — оба вида
//! переводятся в Decimal точно (serde_json собран с arbitrary_precision).
//! Кто вызывает: файлы адаптера BingX.
//! От чего зависит: rust_decimal, serde_json, errors.rs.
//! Настройки: нет.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::{GatewayError, GatewayResult};

use super::errors;

/// Число из поля (строка или число JSON). None — поля нет или null.
/// Ошибка BadResponse — поле есть, но это не число.
pub fn dec_opt(v: &Value, field: &str) -> GatewayResult<Option<Decimal>> {
    match v.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(x) => to_dec(x)
            .map(Some)
            .ok_or_else(|| errors::bad(&format!("поле {field} не число"), &x.to_string())),
    }
}

/// Обязательное число. Ошибка BadResponse — нет поля или не число.
pub fn dec(v: &Value, field: &str) -> GatewayResult<Decimal> {
    dec_opt(v, field)?.ok_or_else(|| errors::bad(&format!("нет поля {field}"), &v.to_string()))
}

/// Значение JSON → Decimal (строка или число, включая экспоненциальную запись).
pub fn to_dec(x: &Value) -> Option<Decimal> {
    let s = match x {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if s.is_empty() {
        return None;
    }
    Decimal::from_str(&s)
        .or_else(|_| Decimal::from_scientific(&s))
        .ok()
        .map(|d| d.normalize())
}

/// Обязательная строка (строка или число JSON → текст). Ошибка BadResponse — нет поля.
pub fn text(v: &Value, field: &str) -> GatewayResult<String> {
    text_opt(v, field).ok_or_else(|| errors::bad(&format!("нет поля {field}"), &v.to_string()))
}

/// Необязательная строка (пустая строка → None).
pub fn text_opt(v: &Value, field: &str) -> Option<String> {
    match v.get(field)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Целое из поля (строка или число). Ошибка BadResponse.
pub fn int(v: &Value, field: &str) -> GatewayResult<i64> {
    let s = text(v, field)?;
    s.parse::<i64>()
        .map_err(|_| errors::bad(&format!("поле {field} не целое"), &s))
}

/// Массив из поля. Ошибка BadResponse — нет поля или не массив.
pub fn arr<'a>(v: &'a Value, field: &str) -> Result<&'a Vec<Value>, GatewayError> {
    v.get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| errors::bad(&format!("нет массива {field}"), &v.to_string()))
}
