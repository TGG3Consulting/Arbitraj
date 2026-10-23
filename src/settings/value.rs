//! Файл: settings/value.rs
//! Что делает: область значения (Scope) и проверка значения настройки по её типу и ограничению.
//! Кто вызывает: settings/store.rs при каждой записи; api/settings.rs (разбор области из запроса).
//! От чего зависит: settings/schema.rs, rust_decimal, reqwest::Url (разбор адреса).
//! Настройки: нет.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::schema::{Kind, Limit, ScopeKind, SettingDef};

/// Конкретная область значения: вся система, площадка, монета или событие.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id")]
pub enum Scope {
    Global,
    Venue(String),
    Coin(String),
    Event(String),
}

impl Scope {
    /// Вид области. Ошибок нет.
    pub fn kind(&self) -> ScopeKind {
        match self {
            Scope::Global => ScopeKind::Global,
            Scope::Venue(_) => ScopeKind::Venue,
            Scope::Coin(_) => ScopeKind::Coin,
            Scope::Event(_) => ScopeKind::Event,
        }
    }

    /// Строка для хранения в SQLite: «global», «venue:bingx», «coin:CAKE», «event:trade_closed».
    pub fn to_key(&self) -> String {
        match self {
            Scope::Global => "global".to_string(),
            Scope::Venue(v) => format!("venue:{v}"),
            Scope::Coin(c) => format!("coin:{c}"),
            Scope::Event(e) => format!("event:{e}"),
        }
    }

    /// Разбор строки из SQLite обратно в область.
    /// Возвращает: Scope или None, если строка не в известном формате.
    pub fn from_key(key: &str) -> Option<Scope> {
        if key == "global" {
            return Some(Scope::Global);
        }
        let (kind, id) = key.split_once(':')?;
        if id.is_empty() {
            return None;
        }
        match kind {
            "venue" => Some(Scope::Venue(id.to_string())),
            "coin" => Some(Scope::Coin(id.to_string())),
            "event" => Some(Scope::Event(id.to_string())),
            _ => None,
        }
    }
}

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
            Ok(Value::from(num))
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
                "http" | "https" | "ws" | "wss" => Ok(Value::String(s.to_string())),
                other => Err(format!(
                    "Схема «{other}» не поддерживается: нужен http(s) или ws(s)"
                )),
            }
        }
        Kind::Ip => {
            let s = value.as_str().ok_or("Ожидается IP-адрес")?.trim();
            s.parse::<std::net::IpAddr>()
                .map(|_| Value::String(s.to_string()))
                .map_err(|_| format!("«{s}» — не IP-адрес"))
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
