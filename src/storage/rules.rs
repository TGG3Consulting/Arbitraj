//! Файл: storage/rules.rs
//! Что делает: хранение правил шлюза в SQLite — список, создание, изменение.
//! Правило хранится целиком как JSON (модель — gateway/rules.rs).
//! Кто вызывает: api/rules.rs; шлюз читает правила при старте и после изменения.
//! От чего зависит: storage/db.rs, gateway/rules.rs.
//! Настройки: нет. Готовых правил нет — все вводятся в интерфейсе.

use chrono::Utc;
use rusqlite::params;

use crate::gateway::rules::Rule;

use super::db::{Db, DbError};

/// Все правила. Ошибки: база, повреждённая запись.
pub async fn list(db: &Db) -> Result<Vec<Rule>, DbError> {
    db.call(|c| {
        let mut st = c.prepare("SELECT id, body FROM rules ORDER BY id")?;
        let rows = st.query_map([], |r| {
            let id: i64 = r.get(0)?;
            let body: String = r.get(1)?;
            let mut rule: Rule = serde_json::from_str(&body).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            rule.id = id;
            Ok(rule)
        })?;
        rows.collect()
    })
    .await
}

/// Создаёт правило. Поле id во входе игнорируется.
/// Возвращает: правило с присвоенным id. Ошибки: база.
pub async fn create(db: &Db, mut rule: Rule) -> Result<Rule, DbError> {
    let body = serde_json::to_string(&rule).map_err(|e| DbError::Json(e.to_string()))?;
    let id = db
        .call(move |c| {
            c.execute(
                "INSERT INTO rules (body, updated_at) VALUES (?1, ?2)",
                params![body, Utc::now().to_rfc3339()],
            )?;
            Ok(c.last_insert_rowid())
        })
        .await?;
    rule.id = id;
    Ok(rule)
}

/// Заменяет правило с id из входа (выключение — это изменение с enabled = false).
/// Возвращает: true — правило найдено и изменено. Ошибки: база.
pub async fn update(db: &Db, rule: Rule) -> Result<bool, DbError> {
    let id = rule.id;
    let body = serde_json::to_string(&rule).map_err(|e| DbError::Json(e.to_string()))?;
    let n = db
        .call(move |c| {
            c.execute(
                "UPDATE rules SET body = ?1, updated_at = ?2 WHERE id = ?3",
                params![body, Utc::now().to_rfc3339(), id],
            )
        })
        .await?;
    Ok(n == 1)
}
