//! Файл: venues/kraken/num.rs
//! Что делает: чтение чисел и полей из ответов Kraken без потери точности. Спот REST и Funding отдают числа строками
//! («decimal string» в OpenAPI https://docs.kraken.com/openapi/spot-rest.yaml), WebSocket v2 — числами JSON («price
//! (float)», https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/book.md): оба вида — в Decimal точно
//! (serde_json с arbitrary_precision хранит запись числа как пришла).
//! Кто вызывает: файлы адаптера Kraken.
//! От чего зависит: rust_decimal, serde_json, errors.rs.
//! Настройки: нет.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::{GatewayError, GatewayResult};

use super::errors;

/// Число из поля (строка или число JSON). None — поля нет, null или пустая строка.
/// Ошибка BadResponse — поле есть, но это не число.
pub fn dec_opt(v: &Value, field: &str) -> GatewayResult<Option<Decimal>> {
    match v.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
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

/// Сумма объекта суммы Funding: {asset, amount} или {asset_amount: {asset, amount}} (оба вида есть в OpenAPI — у
/// fees и у withdrawals). Принимает: объект. Возвращает: (монета, сумма). Ошибка BadResponse.
pub fn amount_of(v: &Value) -> GatewayResult<(String, Decimal)> {
    let a = v.get("asset_amount").unwrap_or(v);
    let name = a
        .get("asset")
        .and_then(|x| text_opt(x, "name"))
        .unwrap_or_default();
    Ok((name, dec(a, "amount")?))
}
