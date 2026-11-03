//! Файл: settings/accounts/db.rs
//! Что делает: загрузка кошельков DEX, маппинга и служебных отметок из SQLite (таблицы dex_wallets, dex_wallet_map,
//! dex_wallet_meta — storage/db.rs) и чтение истории изменений (dex_wallet_history).
//! Кто вызывает: settings/store.rs (Store::load), api/wallets.rs (история).
//! От чего зависит: settings/accounts/mod.rs, storage/db.rs, storage/journal.rs.
//! Настройки: нет.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use rusqlite::params;
use serde_json::{Value, json};

use crate::storage::db::{Db, DbError};
use crate::storage::journal::Journal;

use super::{Accounts, State, Wallet};

/// Отметка «перенос выполнен или не нужен».
pub(crate) const META_MIGRATED: &str = "migrated";
/// id кошелька переноса.
pub(crate) const META_LEGACY: &str = "legacy";
/// Следующий номер id кошелька.
pub(crate) const META_NEXT: &str = "next_id";

impl Accounts {
    /// Загружает всё из базы. Принимает: базу и журнал (в него пишутся изменения). Ошибки: база.
    pub async fn load(db: Db, journal: Journal) -> Result<Self, DbError> {
        let st = db
            .call(|c| {
                let mut wallets = BTreeMap::new();
                let mut q = c.prepare(
                    "SELECT id, name, secret, executor, queue_key, created_at FROM dex_wallets",
                )?;
                let rows = q.query_map([], |r| {
                    Ok(Wallet {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        secret: r.get(2)?,
                        executor: r.get(3)?,
                        queue_key: r.get(4)?,
                        created_at: r.get(5)?,
                    })
                })?;
                for w in rows {
                    let w = w?;
                    wallets.insert(w.id.clone(), w);
                }
                let mut map = BTreeMap::new();
                let mut q = c.prepare("SELECT row, wallet FROM dex_wallet_map")?;
                for r in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))? {
                    let (row, w) = r?;
                    map.insert(row, w);
                }
                let mut meta = BTreeMap::new();
                let mut q = c.prepare("SELECT key, value FROM dex_wallet_meta")?;
                for r in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))? {
                    let (k, v): (String, String) = r?;
                    meta.insert(k, v);
                }
                Ok(State {
                    wallets,
                    map,
                    legacy: meta.get(META_LEGACY).cloned(),
                    migrated: meta.contains_key(META_MIGRATED),
                    next: meta
                        .get(META_NEXT)
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1),
                })
            })
            .await?;
        Ok(Self {
            db,
            journal,
            st: Arc::new(RwLock::new(st)),
            writer: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    /// История изменений кошельков и маппинга, новые первыми. Принимает: сколько записей (задаёт интерфейс).
    /// Ошибки: база.
    pub async fn history(&self, limit: u32) -> Result<Vec<Value>, DbError> {
        self.db
            .call(move |c| {
                let mut st = c.prepare(
                    "SELECT what, target, old_value, new_value, changed_by, changed_at FROM dex_wallet_history
                     ORDER BY id DESC LIMIT ?1",
                )?;
                let it = st.query_map(params![limit], |r| {
                    Ok(json!({
                        "what": r.get::<_, String>(0)?,
                        "target": r.get::<_, String>(1)?,
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

/// Значение из истории (JSON-текст) → JSON; повреждённый — строкой как есть. Ошибок нет.
fn parse(text: Option<String>) -> Value {
    match text {
        Some(t) => serde_json::from_str(&t).unwrap_or(Value::String(t)),
        None => Value::Null,
    }
}
