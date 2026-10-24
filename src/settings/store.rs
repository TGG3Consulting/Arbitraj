//! Файл: settings/store.rs
//! Что делает: хранилище настроек — значения в SQLite + копия в памяти для быстрого чтения,
//! запись изменения в историю и журнал, рассылка изменений на лету.
//! Кто вызывает: api/settings.rs (чтение и запись), все компоненты (чтение через settings/read.rs).
//! От чего зависит: storage/db.rs, storage/journal.rs, settings/catalog.rs, settings/value.rs, settings/live.rs.
//! Настройки: хранит их все. Значений по умолчанию нет: не задано — значит не задано.
//!
//! Изменения выполняются строго по одному (writer): база, кэш и рассылка всегда в одном порядке.
//! История — settings/history.rs.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chrono::Utc;
use rusqlite::params;
use serde_json::{Value, json};
use tokio::sync::{Mutex, broadcast};

use crate::storage::db::{Db, DbError};
use crate::storage::journal::{Journal, Level};

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
    pub(super) db: Db,
    cache: Arc<RwLock<HashMap<Key, Value>>>,
    changes: broadcast::Sender<Change>,
    journal: Journal,
    /// Изменения — строго по одному.
    writer: Arc<Mutex<()>>,
}

impl Store {
    /// Подписка на изменения настроек (ждать смены значения, например паузы). Ошибок нет.
    pub fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    /// Загружает все заданные значения из базы в память.
    /// Принимает: базу, отправителя изменений (его слушают компоненты и интерфейс), журнал.
    /// Ошибки: база. Повреждённые строки пропускаются с записью в лог процесса.
    pub async fn load(
        db: Db,
        changes: broadcast::Sender<Change>,
        journal: Journal,
    ) -> Result<Self, DbError> {
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
                _ => tracing::error!(%name, %scope, "пропущена повреждённая строка настроек"),
            }
        }
        Ok(Self {
            db,
            cache: Arc::new(RwLock::new(map)),
            changes,
            journal,
            writer: Arc::new(Mutex::new(())),
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

    /// Задаёт или очищает значение (None — очистить): база + история, кэш, журнал, рассылка.
    /// Если значение не изменилось — ничего не пишет и возвращает None.
    /// Принимает: имя, область, значение, кто меняет.
    /// Ошибки: неизвестное имя (кроме очистки уже сохранённого значения устаревшей настройки), неверное значение
    /// или область, база.
    pub async fn set(
        &self,
        name: &str,
        scope: Scope,
        value: Option<Value>,
        by: &str,
    ) -> Result<Option<Change>, SetError> {
        // Имени нет в каталоге (настройку удалили из кода, например dex.enabled) — разрешено только очистить
        // оставшееся значение: задать его нельзя (решение 59: устаревшие настройки удаляет человек из интерфейса).
        let new = match (catalog::find(name), value) {
            (Some(def), Some(v)) => Some(validate(def, &scope, &v).map_err(SetError::Invalid)?),
            (_, None)
                if catalog::find(name).is_some() || self.get_exact(name, &scope).is_some() =>
            {
                None
            }
            _ => return Err(SetError::Unknown(name.to_string())),
        };
        let _turn = self.writer.lock().await;
        let old = self.get_exact(name, &scope);
        if old == new {
            return Ok(None);
        }
        let at = Utc::now();
        let (n, sk, by_s, at_s) = (
            name.to_string(),
            scope.to_key(),
            by.to_string(),
            at.to_rfc3339(),
        );
        let (old_t, new_t) = (
            old.as_ref().map(Value::to_string),
            new.as_ref().map(Value::to_string),
        );
        self.db
            .call(move |c| {
                let tx = c.transaction()?;
                match &new_t {
                    Some(v) => tx.execute(
                        "INSERT INTO settings (name, scope, value, updated_at) VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(name, scope) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
                        params![n, sk, v, at_s],
                    )?,
                    None => tx.execute("DELETE FROM settings WHERE name=?1 AND scope=?2", params![n, sk])?,
                };
                tx.execute(
                    "INSERT INTO settings_history (name, scope, old_value, new_value, changed_by, changed_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![n, sk, old_t, new_t, by_s, at_s],
                )?;
                tx.commit()
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
            old,
            new,
            by: by.to_string(),
            at,
        };
        let text = change.describe();
        // Ошибка журнала уже выведена в лог процесса; само изменение сохранено в базе и истории.
        let _ = self
            .journal
            .write(
                Level::Info,
                "settings.changed",
                None,
                text,
                Some(json!(change)),
            )
            .await;
        // Нет слушателей — не ошибка.
        let _ = self.changes.send(change.clone());
        Ok(Some(change))
    }
}
