//! Файл: settings/history.rs
//! Что делает: чтение истории изменений настроек (кто, когда, старое и новое значение).
//! Кто вызывает: api/settings.rs (экран «Настройки» → история).
//! От чего зависит: settings/store.rs, storage/db.rs.
//! Настройки: нет. Сколько записей читать — задаёт интерфейс.

use rusqlite::params;
use serde_json::{Value, json};

use crate::storage::db::DbError;

use super::store::Store;

impl Store {
    /// История изменений, новые первыми.
    /// Принимает: имя (или None — все), сколько записей. Ошибки: база.
    pub async fn history(&self, name: Option<String>, limit: u32) -> Result<Vec<Value>, DbError> {
        self.db
            .call(move |c| {
                let mut st = c.prepare(
                    "SELECT name, scope, old_value, new_value, changed_by, changed_at FROM settings_history
                     WHERE (?1 IS NULL OR name = ?1) ORDER BY id DESC LIMIT ?2",
                )?;
                let it = st.query_map(params![name, limit], |r| {
                    Ok(json!({
                        "name": r.get::<_, String>(0)?,
                        "scope": r.get::<_, String>(1)?,
                        "old": parse(r.get(2)?),
                        "new": parse(r.get(3)?),
                        "by": r.get::<_, String>(4)?,
                        "at": r.get::<_, String>(5)?,
                    }))
                })?;
                it.collect()
            })
            .await
    }
}

/// Значение из истории (JSON-текст) → JSON. Повреждённый текст отдаётся строкой как есть.
fn parse(text: Option<String>) -> Value {
    match text {
        Some(t) => serde_json::from_str(&t).unwrap_or(Value::String(t)),
        None => Value::Null,
    }
}
