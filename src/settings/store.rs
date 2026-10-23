//! Файл: settings/store.rs
//! Что делает: хранилище настроек — значения в SQLite + копия в памяти для быстрого чтения,
//! история изменений (кто, когда, старое и новое значение), рассылка изменений на лету.
//! Кто вызывает: api/settings.rs (чтение и запись), все компоненты (чтение через settings/read.rs).
//! От чего зависит: storage/db.rs, settings/catalog.rs, settings/value.rs, settings/live.rs.
//! Настройки: хранит их все. Значений по умолчанию нет: не задано — значит не задано.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::storage::db::{Db, DbError};

use super::catalog;
use super::live::Change;
use super::value::{Scope, validate};

/// Ошибка записи настройки.
#[derive(Debug, thiserror::Error)]
pub enum SetError {
    #[error("Настройки «{0}» нет в каталоге")]
    Unknown(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Db(#[from] DbError),
}

type Key = (String, Scope);

/// Хранилище настроек. Дешёво клонируется.
#[derive(Clone)]
pub struct Store {
    db: Db,
    cache: Arc<RwLock<HashMap<Key, Value>>>,
    changes: broadcast::Sender<Change>,
}

impl Store {
    /// Загружает все заданные значения из базы в память.
    /// Принимает: базу и отправителя изменений (его слушают компоненты и интерфейс).
    /// Ошибки: база. Строки с неизвестной областью пропускаются с предупреждением.
    pub async fn load(db: Db, changes: broadcast::Sender<Change>) -> Result<Self, DbError> {
        let rows: Vec<(String, String, String)> = db
            .call(|c| {
                let mut st = c.prepare("SELECT name, scope, value FROM settings")?;
                let it = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                it.collect()
            })
            .await?;
        let mut map = HashMap::new();
        for (name, scope, value) in rows {
            match (
                Scope::from_key(&scope),
                serde_json::from_str::<Value>(&value),
            ) {
                (Some(s), Ok(v)) => {
                    map.insert((name, s), v);
                }
                _ => tracing::warn!(%name, %scope, "пропущена повреждённая строка настроек"),
            }
        }
        Ok(Self {
            db,
            cache: Arc::new(RwLock::new(map)),
            changes,
        })
    }

    /// Значение ровно в этой области (без подстановки из общей).
    /// Возвращает: значение или None — не задано.
    pub fn get_exact(&self, name: &str, scope: &Scope) -> Option<Value> {
        let cache = self.cache.read().unwrap_or_else(|p| p.into_inner());
        cache.get(&(name.to_string(), scope.clone())).cloned()
    }

    /// Действующее значение: монета сильнее площадки, площадка сильнее всей системы.
    /// Принимает: имя, площадку и монету (если применимо). Возвращает: значение или None.
    pub fn resolve(&self, name: &str, venue: Option<&str>, coin: Option<&str>) -> Option<Value> {
        let order = [
            coin.map(|c| Scope::Coin(c.to_string())),
            venue.map(|v| Scope::Venue(v.to_string())),
            Some(Scope::Global),
        ];
        order
            .into_iter()
            .flatten()
            .find_map(|s| self.get_exact(name, &s))
    }

    /// Все заданные значения — для экрана «Настройки».
    pub fn all(&self) -> Vec<(String, Scope, Value)> {
        let cache = self.cache.read().unwrap_or_else(|p| p.into_inner());
        cache
            .iter()
            .map(|((n, s), v)| (n.clone(), s.clone(), v.clone()))
            .collect()
    }

    /// Задаёт или очищает значение (None — очистить), пишет историю, рассылает изменение.
    /// Принимает: имя, область, значение, кто меняет. Ошибки: неизвестное имя, неверное значение, база.
    pub async fn set(
        &self,
        name: &str,
        scope: Scope,
        value: Option<Value>,
        by: &str,
    ) -> Result<Change, SetError> {
        let def = catalog::find(name).ok_or_else(|| SetError::Unknown(name.to_string()))?;
        let new = match value {
            Some(v) => Some(validate(def, &scope, &v).map_err(SetError::Invalid)?),
            None => None,
        };
        let (n, sk, by_s) = (name.to_string(), scope.to_key(), by.to_string());
        let new_text = new.as_ref().map(|v| v.to_string());
        let old_text: Option<String> = self
            .db
            .call(move |c| {
                let tx = c.transaction()?;
                let now = Utc::now().to_rfc3339();
                let old: Option<String> = tx
                    .query_row("SELECT value FROM settings WHERE name=?1 AND scope=?2", params![n, sk], |r| r.get(0))
                    .optional()?;
                match &new_text {
                    Some(v) => tx.execute(
                        "INSERT INTO settings (name, scope, value, updated_at) VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(name, scope) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
                        params![n, sk, v, now],
                    )?,
                    None => tx.execute("DELETE FROM settings WHERE name=?1 AND scope=?2", params![n, sk])?,
                };
                tx.execute(
                    "INSERT INTO settings_history (name, scope, old_value, new_value, changed_by, changed_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![n, sk, old, new_text, by_s, now],
                )?;
                tx.commit()?;
                Ok(old)
            })
            .await?;
        {
            let mut cache = self.cache.write().unwrap_or_else(|p| p.into_inner());
            let key = (name.to_string(), scope.clone());
            match &new {
                Some(v) => cache.insert(key, v.clone()),
                None => cache.remove(&key),
            };
        }
        let change = Change {
            name: name.to_string(),
            scope,
            old: old_text.and_then(|t| serde_json::from_str(&t).ok()),
            new,
            by: by.to_string(),
            at: Utc::now(),
        };
        // Нет слушателей — не ошибка.
        let _ = self.changes.send(change.clone());
        Ok(change)
    }

    /// История изменений, новые первыми.
    /// Принимает: имя (или None — все), сколько записей (задаёт интерфейс). Ошибки: база.
    pub async fn history(&self, name: Option<String>, limit: u32) -> Result<Vec<Value>, DbError> {
        self.db
            .call(move |c| {
                let mut st = c.prepare(
                    "SELECT name, scope, old_value, new_value, changed_by, changed_at FROM settings_history
                     WHERE (?1 IS NULL OR name = ?1) ORDER BY id DESC LIMIT ?2",
                )?;
                let it = st.query_map(params![name, limit], |r| {
                    Ok(serde_json::json!({
                        "name": r.get::<_, String>(0)?, "scope": r.get::<_, String>(1)?,
                        "old": parse(r.get::<_, Option<String>>(2)?), "new": parse(r.get::<_, Option<String>>(3)?),
                        "by": r.get::<_, String>(4)?, "at": r.get::<_, String>(5)?,
                    }))
                })?;
                it.collect()
            })
            .await
    }
}

/// Значение из истории (JSON-текст) → JSON; повреждённый текст отдаётся как есть строкой.
fn parse(text: Option<String>) -> Value {
    match text {
        Some(t) => serde_json::from_str(&t).unwrap_or(Value::String(t)),
        None => Value::Null,
    }
}
