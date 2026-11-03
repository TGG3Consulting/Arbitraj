//! Файл: settings/accounts/edit.rs
//! Что делает: изменения кошельков DEX и маппинга (решение 132): добавить, переименовать, задать исполнитель, удалить
//! (запрещено, пока кошелёк в маппинге или в незавершённой сделке), привязать строку маппинга. Каждое изменение —
//! строго по одному: база (строка + история «было/стало», кто, когда) одной транзакцией → память → журнал
//! (вид wallets.changed — по нему экран перечитывает данные). Ключей здесь нет: только имя секрета.
//! Кто вызывает: api/wallets.rs, settings/accounts/migrate.rs.
//! От чего зависит: settings/accounts/{mod,db,edit_map}.rs (помощники), storage/db.rs, storage/journal.rs.
//! Настройки: нет.

use chrono::Utc;
use rusqlite::params;
use serde_json::{Value, json};

use crate::storage::db::DbError;

use super::{Accounts, Wallet};

/// Вид записи журнала об изменении кошельков и маппинга.
pub const JOURNAL_KIND: &str = "wallets.changed";

/// Ошибка изменения.
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Db(#[from] DbError),
}

/// Одна строка истории: что, над чем, было, стало.
pub(crate) struct Hist {
    pub what: &'static str,
    pub target: String,
    pub old: Option<Value>,
    pub new: Option<Value>,
}

impl Accounts {
    /// Добавляет кошелёк (ключ уже записан в секреты вызывающим под именем secret). Принимает: id (reserve_id), имя,
    /// имя секрета, ключ очереди, кто. Ошибки: Invalid (пустое или занятое имя, id занят), база.
    pub async fn add(
        &self,
        id: &str,
        name: &str,
        secret: &str,
        queue_key: &str,
        by: &str,
    ) -> Result<Wallet, EditError> {
        let _turn = self.writer.lock().await;
        let name = self.check_name(name, None)?;
        if self.wallet(id).is_some() {
            return Err(EditError::Invalid(format!("Кошелёк {id} уже есть")));
        }
        let w = Wallet {
            id: id.to_string(),
            name,
            secret: secret.to_string(),
            executor: None,
            queue_key: queue_key.to_string(),
            created_at: Utc::now().to_rfc3339(),
        };
        self.insert_locked(w, by).await
    }

    /// Переименовывает. Ошибки: Invalid (нет кошелька, пустое или занятое имя), база.
    pub async fn rename(&self, id: &str, name: &str, by: &str) -> Result<Wallet, EditError> {
        let _turn = self.writer.lock().await;
        let mut w = self.must(id)?;
        let name = self.check_name(name, Some(id))?;
        let h = Hist {
            what: "wallet.rename",
            target: id.to_string(),
            old: Some(json!(w.name)),
            new: Some(json!(name)),
        };
        let (i, n) = (id.to_string(), name.clone());
        self.commit(h, by, move |tx| {
            tx.execute(
                "UPDATE dex_wallets SET name = ?2 WHERE id = ?1",
                params![i, n],
            )
            .map(|_| ())
        })
        .await?;
        let text = format!("Кошелёк DEX «{}» переименован в «{name}» ({by})", w.name);
        w.name = name;
        self.put(w.clone());
        self.say(text).await;
        Ok(w)
    }

    /// Задаёт или снимает адрес исполнителя ArbExecutor кошелька (проверка адреса и владельца — у вызывающего).
    /// Принимает: id, адрес (0x… или None), кто. Ошибки: Invalid (нет кошелька), база.
    pub async fn set_executor(
        &self,
        id: &str,
        executor: Option<String>,
        by: &str,
    ) -> Result<Wallet, EditError> {
        let _turn = self.writer.lock().await;
        self.executor_locked(id, executor, by).await
    }

    /// set_executor без замка (замок взят вызывающим — перенос). Ошибки: как у set_executor.
    pub(crate) async fn executor_locked(
        &self,
        id: &str,
        executor: Option<String>,
        by: &str,
    ) -> Result<Wallet, EditError> {
        let mut w = self.must(id)?;
        if w.executor == executor {
            return Ok(w);
        }
        let h = Hist {
            what: "wallet.executor",
            target: id.to_string(),
            old: w.executor.as_ref().map(|x| json!(x)),
            new: executor.as_ref().map(|x| json!(x)),
        };
        let (i, x) = (id.to_string(), executor.clone());
        self.commit(h, by, move |tx| {
            tx.execute(
                "UPDATE dex_wallets SET executor = ?2 WHERE id = ?1",
                params![i, x],
            )
            .map(|_| ())
        })
        .await?;
        let shown = executor.clone().unwrap_or_else(|| "не задан".into());
        let text = format!("Кошелёк DEX «{}»: исполнитель {shown} ({by})", w.name);
        w.executor = executor;
        self.put(w.clone());
        self.say(text).await;
        Ok(w)
    }

    /// Удаляет запись кошелька. Секрет ключа НЕ удаляется (на кошельке могут быть деньги — ключ убирает человек на
    /// «Разблокировке»). Принимает: id, незавершённые сделки этого кошелька (считает ядро), кто.
    /// Ошибки: Invalid (нет кошелька; в маппинге; в незавершённой сделке), база.
    pub async fn remove(&self, id: &str, busy: &[String], by: &str) -> Result<Wallet, EditError> {
        let _turn = self.writer.lock().await;
        let w = self.must(id)?;
        let rows = self.rows_of(id);
        if !rows.is_empty() {
            return Err(EditError::Invalid(format!(
                "Кошелёк «{}» привязан в маппинге ({}) — сначала отвяжите",
                w.name,
                rows.join(", ")
            )));
        }
        if !busy.is_empty() {
            return Err(EditError::Invalid(format!(
                "Кошелёк «{}» ведёт незавершённые сделки ({}) — удалить нельзя",
                w.name,
                busy.join(", ")
            )));
        }
        let h = Hist {
            what: "wallet.remove",
            target: id.to_string(),
            old: Some(json!({"name": w.name, "secret": w.secret, "executor": w.executor})),
            new: None,
        };
        let i = id.to_string();
        self.commit(h, by, move |tx| {
            tx.execute("DELETE FROM dex_wallets WHERE id = ?1", params![i])
                .map(|_| ())
        })
        .await?;
        self.st
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .wallets
            .remove(id);
        let text = format!(
            "Кошелёк DEX «{}» удалён ({by}); ключ {} остался в секретах",
            w.name, w.secret
        );
        self.say(text).await;
        Ok(w)
    }
}
