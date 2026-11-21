//! Файл: venues/phemex/num.rs
//! Что делает: чтение полей ответов Phemex без потери точности и перевод масштабированных целых в Decimal и обратно.
//! Phemex передаёт цены, доли и суммы масштабированными целыми: «…Ep» — цена × 10^priceScale, «…Er» — доля × 10^8,
//! «…Ev» — значение × 10^valueScale монеты; «…Rv» — настоящее значение строкой (раздел вывода и депозита). Ошибка
//! масштаба = ордер в 10^k раз больше или меньше, поэтому перевод только целочисленный (без f64), а обратный перевод
//! отказывает, если значение не ложится на масштаб ровно.
//! Кто вызывает: файлы адаптера Phemex.
//! От чего зависит: rust_decimal, serde_json, errors.rs.
//! Настройки: нет.
//!
//! Официально: Spot REST API → Price/Ratio/Value scales (https://phemex-docs.github.io/#price-ratio-value-scales):
//! «Fields with post-fix "Ep" are scaled prices, priceScale», «"Er" are scaled ratios, ratioScale», «"Ev" are scaled
//! values, valueScale», «ratioScale is always scaled 8»; Deposit And Withdraw → «Fields with post-fix "Rv" means real
//! value without being scaled» (https://phemex-docs.github.io/#price-ratio-value).

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::{GatewayError, GatewayResult};

use super::errors;

/// Масштаб долей «…Er» — официально «ratioScale is always scaled 8».
pub const RATIO_SCALE: u32 = 8;
/// Наибольший масштаб Decimal.
const MAX_SCALE: u32 = 28;

/// Целое JSON (число или строка с целым) → i128. None — не целое. Ошибок нет.
pub fn to_int(x: &Value) -> Option<i128> {
    match x {
        Value::Number(n) => n.to_string().parse().ok(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Масштабированное целое → настоящее значение: v / 10^scale (точно). None — масштаб больше 28. Ошибок нет.
pub fn unscale(v: i128, scale: u32) -> Option<Decimal> {
    if scale > MAX_SCALE {
        return None;
    }
    Decimal::try_from_i128_with_scale(v, scale)
        .ok()
        .map(|d| d.normalize())
}

/// Настоящее значение → масштабированное целое: d × 10^scale. Ошибка — текст: значение не ложится на масштаб ровно
/// (лишние знаки) или не помещается в i64.
pub fn scale_up(d: Decimal, scale: u32) -> Result<i64, String> {
    if scale > MAX_SCALE {
        return Err(format!("масштаб {scale} больше {MAX_SCALE}"));
    }
    let mut m = d.normalize();
    if m.scale() > scale {
        return Err(format!("{m} не ложится на масштаб 10^{scale}"));
    }
    m.rescale(scale);
    i64::try_from(m.mantissa()).map_err(|_| format!("{d} × 10^{scale} не помещается в целое"))
}

/// Обязательное поле «…Ev/…Ep/…Er» → настоящее значение по масштабу. Ошибка BadResponse — нет поля или не целое.
pub fn scaled(v: &Value, field: &str, scale: u32) -> GatewayResult<Decimal> {
    scaled_opt(v, field, scale)?
        .ok_or_else(|| errors::bad(&format!("нет поля {field}"), &v.to_string()))
}

/// Необязательное поле «…Ev/…Ep/…Er» (нет или null — None). Ошибка BadResponse — не целое или масштаб велик.
pub fn scaled_opt(v: &Value, field: &str, scale: u32) -> GatewayResult<Option<Decimal>> {
    match v.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(x) => to_int(x)
            .and_then(|i| unscale(i, scale))
            .map(Some)
            .ok_or_else(|| errors::bad(&format!("поле {field} не целое"), &x.to_string())),
    }
}

/// Число «…Rv» или любое число (строка или число JSON). None — поля нет, null или пустая строка.
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

/// Значение JSON → Decimal (строка или число, включая экспоненциальную запись). Ошибок нет.
pub fn to_dec(x: &Value) -> Option<Decimal> {
    let s = match x {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    Decimal::from_str(&s)
        .or_else(|_| Decimal::from_scientific(&s))
        .ok()
        .map(|d| d.normalize())
}

/// Обязательная строка (строка или число JSON → текст). Ошибка BadResponse — нет поля.
pub fn text(v: &Value, field: &str) -> GatewayResult<String> {
    text_opt(v, field).ok_or_else(|| errors::bad(&format!("нет поля {field}"), &v.to_string()))
}

/// Необязательная строка (пустая строка → None). Ошибок нет.
pub fn text_opt(v: &Value, field: &str) -> Option<String> {
    match v.get(field)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Целое (масштаб, счётчик) из поля. Ошибка BadResponse — нет поля или не целое.
pub fn int(v: &Value, field: &str) -> GatewayResult<i128> {
    v.get(field)
        .and_then(to_int)
        .ok_or_else(|| errors::bad(&format!("поле {field} не целое"), &v.to_string()))
}

/// Масштаб (0…28) из поля. Ошибка BadResponse.
pub fn scale_of(v: &Value, field: &str) -> GatewayResult<u32> {
    let n = int(v, field)?;
    u32::try_from(n)
        .ok()
        .filter(|s| *s <= MAX_SCALE)
        .ok_or_else(|| errors::bad(&format!("{field} = {n} вне 0…28"), &v.to_string()))
}

/// Массив из поля. Ошибка BadResponse — нет поля или не массив.
pub fn arr<'a>(v: &'a Value, field: &str) -> Result<&'a Vec<Value>, GatewayError> {
    v.get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| errors::bad(&format!("нет массива {field}"), &v.to_string()))
}

/// Список записей ответа: data — массив, или объект с массивом rows (так выглядит «Query funds history»). Пустой
/// ответ (null) — пусто. Ошибка BadResponse — другое.
pub fn rows(data: Value, what: &str) -> GatewayResult<Vec<Value>> {
    match data {
        Value::Array(a) => Ok(a),
        Value::Null => Ok(vec![]),
        Value::Object(mut m) => match m.remove("rows") {
            Some(Value::Array(a)) => Ok(a),
            Some(Value::Null) | None if m.is_empty() => Ok(vec![]),
            other => Err(errors::bad(
                &format!("{what}: data не массив и не {{rows: […]}}"),
                &other.map(|o| o.to_string()).unwrap_or_default(),
            )),
        },
        other => Err(errors::bad(
            &format!("{what}: data не массив"),
            &other.to_string(),
        )),
    }
}
