//! Файл: storage/rules.rs
//! Что делает: хранение правил шлюза в SQLite — список, создание, изменение.
//! Правило хранится целиком как JSON (модель — gateway/rules.rs).
//! Решение 354: снимок правил в памяти (Db::rules_cache) — list без SQLite; снимок ставится при открытии базы и
//! сразу после каждого изменения (create, update) под замком основной базы.
//! Кто вызывает: api/rules.rs; core/rules_apply.rs (ошибка на пути сделки), venues/bsc_dex/node/pace.rs; storage/db.rs
//! (read_all — снимок при открытии).
//! От чего зависит: storage/db.rs, gateway/rules.rs.
//! Настройки: нет. Готовых правил нет — все вводятся в интерфейсе.

use chrono::Utc;
use rusqlite::params;

use crate::gateway::rules::Rule;

use super::db::{Db, DbError, Rules};

/// Все правила. Ошибки: база, повреждённая запись.
/// Решение 354: из снимка в памяти (Db::rules_cache) — без обращения к SQLite (правила читает путь сделки при ошибке,
/// core/rules_apply.rs, и узел DEX при 429, node/pace.rs). Снимка нет (первое чтение или прошлое чтение с ошибкой) —
/// чтение из базы; снимок ставится в той же функции над соединением, под замком основной базы, — по порядку с
/// изменениями (create, update ставят снимок там же), устареть он не может.
pub async fn list(db: &Db) -> Result<Vec<Rule>, DbError> {
    if let Some(v) = cached(db) {
        return Ok(v);
    }
    let cache = db.rules_cache().clone();
    db.call(move |c| {
        let v = read_all(c)?;
        *cache.write().unwrap_or_else(|p| p.into_inner()) = Some(v.clone());
        Ok(v)
    })
    .await
}

/// Снимок правил, если есть. Ошибок нет.
fn cached(db: &Db) -> Option<Vec<Rule>> {
    db.rules_cache()
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

/// Что делает: все правила из таблицы rules (синхронно, над соединением). Принимает: соединение.
/// Возвращает: правила по порядку id. Ошибки: SQLite, повреждённая запись.
pub(super) fn read_all(c: &rusqlite::Connection) -> rusqlite::Result<Vec<Rule>> {
    let mut st = c.prepare("SELECT id, body FROM rules ORDER BY id")?;
    let rows = st.query_map([], |r| {
        let id: i64 = r.get(0)?;
        let body: String = r.get(1)?;
        let mut rule: Rule = serde_json::from_str(&body).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?;
        rule.id = id;
        Ok(rule)
    })?;
    rows.collect()
}

/// Создаёт правило. Поле id во входе игнорируется.
/// Возвращает: правило с присвоенным id. Ошибки: база.
pub async fn create(db: &Db, mut rule: Rule) -> Result<Rule, DbError> {
    let body = serde_json::to_string(&rule).map_err(|e| DbError::Json(e.to_string()))?;
    let cache = db.rules_cache().clone();
    let id = db
        .call(move |c| {
            c.execute(
                "INSERT INTO rules (body, updated_at) VALUES (?1, ?2)",
                params![body, Utc::now().to_rfc3339()],
            )?;
            let id = c.last_insert_rowid();
            // Решение 354: снимок — сразу после изменения, под тем же замком.
            refresh(&cache, c);
            Ok(id)
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
    let cache = db.rules_cache().clone();
    let n = db
        .call(move |c| {
            let n = c.execute(
                "UPDATE rules SET body = ?1, updated_at = ?2 WHERE id = ?3",
                params![body, Utc::now().to_rfc3339(), id],
            )?;
            // Решение 354: снимок — сразу после изменения, под тем же замком.
            refresh(&cache, c);
            Ok(n)
        })
        .await?;
    Ok(n == 1)
}

/// Решение 354: перечитывает снимок правил после изменения (под замком основной базы). Не прочиталось — снимок
/// снимается (следующее чтение пойдёт в базу и вернёт ту же ошибку, что раньше). Принимает: снимок, соединение.
/// Ошибок нет.
fn refresh(cache: &Rules, c: &rusqlite::Connection) {
    let v = read_all(c).ok();
    *cache.write().unwrap_or_else(|p| p.into_inner()) = v;
}
