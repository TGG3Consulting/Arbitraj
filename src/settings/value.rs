//! Файл: settings/value.rs
//! Что делает: проверка значения настройки по её типу, ограничению, вариантам и верхней границе.
//! Область значения (Scope) — в settings/scope.rs, здесь переэкспортируется для старых путей.
//! Кто вызывает: settings/store.rs при каждой записи; api/settings.rs (разбор области из запроса).
//! От чего зависит: settings/schema.rs, settings/scope.rs, rust_decimal, reqwest::Url, alloy-primitives.
//! Настройки: нет.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::Value;

use super::schema::{Kind, Limit, SettingDef};
pub use super::scope::Scope;

/// Проверяет значение по описанию настройки.
/// Принимает: описание, область, значение (JSON из интерфейса).
/// Возвращает: Ok(значение в форме хранения) или Err(понятная причина по-русски).
/// Десятичные числа приводятся к строке, чтобы не терять точность.
pub fn validate(def: &SettingDef, scope: &Scope, value: &Value) -> Result<Value, String> {
    if !def.scopes.contains(&scope.kind()) {
        return Err(format!(
            "Настройка {} не задаётся для области {:?}",
            def.name,
            scope.kind()
        ));
    }
    check_scope_id(scope)?;
    match def.kind {
        Kind::Decimal => {
            let text = match value {
                Value::String(s) => s.trim().to_string(),
                Value::Number(n) => n.to_string(),
                _ => return Err("Ожидается число".into()),
            };
            let num = Decimal::from_str(&text).map_err(|_| format!("«{text}» — не число"))?;
            check_limit(def.limit, num)?;
            Ok(Value::String(num.normalize().to_string()))
        }
        Kind::Integer => {
            let num = value
                .as_u64()
                .ok_or("Ожидается целое неотрицательное число")?;
            check_limit(def.limit, Decimal::from(num))?;
            match def.max {
                Some(max) if num > max => Err(format!("Значение {num} больше допустимого {max}")),
                _ => Ok(Value::from(num)),
            }
        }
        Kind::Bool => value
            .as_bool()
            .map(Value::from)
            .ok_or_else(|| "Ожидается да или нет".into()),
        Kind::Text => match value.as_str().map(str::trim) {
            Some(s) if !s.is_empty() => Ok(Value::String(s.to_string())),
            _ => Err("Ожидается непустой текст".into()),
        },
        Kind::Url => {
            let s = value.as_str().ok_or("Ожидается адрес")?.trim();
            let url = reqwest::Url::parse(s).map_err(|e| format!("Неверный адрес: {e}"))?;
            match url.scheme() {
                // Только защищённые соединения: в адресе может быть ключ или токен.
                "https" | "wss" => Ok(Value::String(s.to_string())),
                other => Err(format!(
                    "Схема «{other}» не подходит: нужно защищённое соединение https или wss"
                )),
            }
        }
        Kind::Ip => {
            let s = value.as_str().ok_or("Ожидается IP-адрес")?.trim();
            s.parse::<std::net::IpAddr>()
                .map(|_| Value::String(s.to_string()))
                .map_err(|_| format!("«{s}» — не IP-адрес"))
        }
        Kind::Choice => match value.as_str().map(str::trim) {
            Some(c) if def.choices.contains(&c) => Ok(Value::String(c.to_string())),
            _ => Err(format!(
                "Ожидается один из вариантов: {}",
                def.choices.join(", ")
            )),
        },
        Kind::Address => {
            let s = value.as_str().ok_or("Ожидается адрес 0x…")?.trim();
            let ok = s.len() == 42
                && s.starts_with("0x")
                && s.parse::<alloy_primitives::Address>().is_ok();
            if ok {
                Ok(Value::String(s.to_string()))
            } else {
                Err(format!(
                    "«{s}» — не адрес сети BSC (0x и 40 шестнадцатеричных знаков)"
                ))
            }
        }
        Kind::AddressList => {
            let arr = value.as_array().ok_or("Ожидается список адресов 0x…")?;
            let mut out = Vec::new();
            for v in arr {
                let a = v.as_str().map(str::trim).unwrap_or_default();
                let ok = a.len() == 42
                    && a.starts_with("0x")
                    && a.parse::<alloy_primitives::Address>().is_ok();
                if !ok {
                    return Err(format!(
                        "«{a}» — не адрес сети BSC (0x и 40 шестнадцатеричных знаков)"
                    ));
                }
                if !out.contains(&Value::String(a.to_string())) {
                    out.push(Value::String(a.to_string()));
                }
            }
            Ok(Value::Array(out))
        }
        Kind::TextList => {
            let arr = value.as_array().ok_or("Ожидается список")?;
            let items: Option<Vec<Value>> = arr
                .iter()
                .map(|v| v.as_str().map(|s| Value::String(s.trim().to_string())))
                .collect();
            items
                .map(Value::Array)
                .ok_or_else(|| "Все элементы списка должны быть текстом".into())
        }
    }
}

/// Проверка области: идентификатор не пустой, событие есть в каталоге уведомлений.
/// Возвращает Err с причиной, если область неверная.
fn check_scope_id(scope: &Scope) -> Result<(), String> {
    match scope {
        Scope::Global => Ok(()),
        Scope::Venue(id) | Scope::Coin(id) | Scope::Dex(id) | Scope::Channel(id)
            if id.trim().is_empty() =>
        {
            Err("Не указан идентификатор области".into())
        }
        Scope::Event(code) if crate::notify::catalog::find(code).is_none() => {
            Err(format!("События «{code}» нет в каталоге уведомлений"))
        }
        _ => Ok(()),
    }
}

/// Проверка числа по ограничению. Возвращает Err с причиной, если число вне допустимого.
fn check_limit(limit: Limit, n: Decimal) -> Result<(), String> {
    let ok = match limit {
        Limit::None => true,
        Limit::Positive => n > Decimal::ZERO,
        Limit::NonNegative => n >= Decimal::ZERO,
        Limit::Fraction => n >= Decimal::ZERO && n <= Decimal::ONE,
        Limit::Percent => n >= Decimal::ZERO && n <= Decimal::ONE_HUNDRED,
    };
    if ok {
        Ok(())
    } else {
        let rule = match limit {
            Limit::None => "любое",
            Limit::Positive => "больше нуля",
            Limit::NonNegative => "ноль или больше",
            Limit::Fraction => "доля от 0 до 1",
            Limit::Percent => "процент от 0 до 100",
        };
        Err(format!("Значение {n} не подходит: нужно {rule}"))
    }
}
