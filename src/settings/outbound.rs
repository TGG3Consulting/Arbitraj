//! Настройки исходящих Telegram-рассылок. UTC без локального смещения, числовые ID без потери точности.
use chrono::NaiveTime;
use serde_json::Value;

/// Проверяем тот же формат при сохранении и при назначении следующей рассылки.
pub fn utc_time(text: &str) -> Option<NaiveTime> {
    let (hour, minute) = text.split_once(':')?;
    if hour.len() != 2
        || minute.len() != 2
        || !hour
            .bytes()
            .chain(minute.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    NaiveTime::from_hms_opt(hour.parse().ok()?, minute.parse().ok()?, 0)
}

/// В Telegram ID группы может быть отрицательным. Повтор ID означает одного получателя.
pub(super) fn chat_ids(value: &Value) -> Result<Value, String> {
    let ids = value.as_array().ok_or("Укажите список числовых ID чатов")?;
    let mut result = Vec::new();
    for item in ids {
        let id = item
            .as_str()
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|n| *n != 0)
            .ok_or("Укажите числовой ID каждого чата, отличный от нуля")?;
        let id = Value::String(id.to_string());
        if !result.contains(&id) {
            result.push(id);
        }
    }
    Ok(Value::Array(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn utc_requires_hours_and_minutes() {
        for time in ["00:00", "09:05", "23:59"] {
            assert!(utc_time(time).is_some());
        }
        for time in ["24:00", "23:60", "9:05", "+9:00", "12:30:00"] {
            assert!(utc_time(time).is_none());
        }
    }

    #[test]
    fn recipients_are_numeric_and_unique() {
        assert_eq!(
            chat_ids(&json!(["-100123", " 123 ", "00123"])).unwrap(),
            json!(["-100123", "123"])
        );
        assert!(chat_ids(&json!(["-100123", "x"])).is_err());
        assert!(chat_ids(&json!(["0"])).is_err());
        assert_eq!(chat_ids(&json!([])).unwrap(), json!([]));
    }
}
