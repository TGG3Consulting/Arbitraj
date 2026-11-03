//! Файл: settings/accounts/edit_map.rs
//! Что делает: привязка строки маппинга к кошельку (решение 132) и общие помощники изменений: проверка имени, запись
//! строки истории в одной транзакции с изменением, обновление памяти, запись в журнал.
//! Кто вызывает: settings/accounts/edit.rs, settings/accounts/migrate.rs, api/wallets.rs (map_set).
//! От чего зависит: settings/accounts/{mod,edit}.rs, storage/db.rs, storage/journal.rs.
//! Настройки: нет.

use chrono::Utc;
use rusqlite::{Transaction, params};
use serde_json::json;

use crate::storage::journal::Level;

use super::db::META_NEXT;
use super::edit::{EditError, Hist, JOURNAL_KIND};
use super::{Accounts, Wallet};

/// Имя секрета ключа нового кошелька в зашифрованном файле (правило имени секрета — secrets/vault.rs). Ошибок нет.
pub fn secret_name(id: &str) -> String {
    format!("wallet.{id}.key")
}

impl Accounts {
    /// Выдаёт id нового кошелька («w<номер>»; номер не переиспользуется). Ошибки: база.
    pub async fn reserve_id(&self) -> Result<String, EditError> {
        let _turn = self.writer.lock().await;
        self.reserve_locked().await
    }

    /// reserve_id без замка (замок взят вызывающим). Ошибки: база.
    pub(crate) async fn reserve_locked(&self) -> Result<String, EditError> {
        let n = self.read().next;
        let next = (n + 1).to_string();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO dex_wallet_meta (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![META_NEXT, next],
                )
                .map(|_| ())
            })
            .await?;
        self.st.write().unwrap_or_else(|p| p.into_inner()).next = n + 1;
        Ok(format!("w{n}"))
    }

    /// Вставляет кошелёк (замок взят вызывающим): база + история, память, журнал. Ошибки: база.
    pub(crate) async fn insert_locked(&self, w: Wallet, by: &str) -> Result<Wallet, EditError> {
        let row = w.clone();
        let h = Hist {
            what: "wallet.add",
            target: w.id.clone(),
            old: None,
            new: Some(json!({"name": w.name, "secret": w.secret})),
        };
        self.commit(h, by, move |tx| {
            tx.execute(
                "INSERT INTO dex_wallets (id, name, secret, executor, queue_key, created_at)
                 VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
                params![row.id, row.name, row.secret, row.queue_key, row.created_at],
            )
            .map(|_| ())
        })
        .await?;
        self.put(w.clone());
        self.say(format!("Кошелёк DEX «{}» добавлен ({by})", w.name))
            .await;
        Ok(w)
    }

    /// Привязывает строку маппинга к кошельку или отвязывает (None). Принимает: строку (id биржи, ROW_DEX_DEX,
    /// ROW_PROBE — список строк задаёт вызывающий), id кошелька, кто. Возвращает: изменилось ли.
    /// Ошибки: Invalid (пустая строка, нет кошелька), база.
    pub async fn map_set(
        &self,
        row: &str,
        wallet: Option<&str>,
        by: &str,
    ) -> Result<bool, EditError> {
        let _turn = self.writer.lock().await;
        self.map_locked(row, wallet, by).await
    }

    /// map_set без замка (замок уже взят вызывающим — перенос). Ошибки: как у map_set.
    pub(crate) async fn map_locked(
        &self,
        row: &str,
        wallet: Option<&str>,
        by: &str,
    ) -> Result<bool, EditError> {
        if row.trim().is_empty() {
            return Err(EditError::Invalid("Не указана строка маппинга".into()));
        }
        let name = |id: &str| self.wallet(id).map(|w| w.name).unwrap_or_else(|| id.into());
        if let Some(id) = wallet {
            self.must(id)?;
        }
        let old = self.row(row);
        if old.as_deref() == wallet {
            return Ok(false);
        }
        let h = Hist {
            what: "map",
            target: row.to_string(),
            old: old.as_ref().map(|x| json!(x)),
            new: wallet.map(|x| json!(x)),
        };
        let (r, w, at) = (
            row.to_string(),
            wallet.map(str::to_string),
            Utc::now().to_rfc3339(),
        );
        self.commit(h, by, move |tx| {
            match &w {
                Some(w) => tx.execute(
                    "INSERT INTO dex_wallet_map (row, wallet, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(row) DO UPDATE SET wallet = excluded.wallet, updated_at = excluded.updated_at",
                    params![r, w, at],
                ),
                None => tx.execute("DELETE FROM dex_wallet_map WHERE row = ?1", params![r]),
            }
            .map(|_| ())
        })
        .await?;
        let text = format!(
            "Маппинг кошельков: {row} — было {}, стало {} ({by})",
            old.as_deref().map(name).unwrap_or_else(|| "нет".into()),
            wallet.map(name).unwrap_or_else(|| "нет".into())
        );
        {
            let mut st = self.st.write().unwrap_or_else(|p| p.into_inner());
            match wallet {
                Some(id) => st.map.insert(row.to_string(), id.to_string()),
                None => st.map.remove(row),
            };
        }
        self.say(text).await;
        Ok(true)
    }

    /// Изменение в базе и строка истории — одной транзакцией. Принимает: строку истории, кто, само изменение.
    /// Ошибки: база.
    pub(crate) async fn commit<F>(&self, h: Hist, by: &str, f: F) -> Result<(), EditError>
    where
        F: FnOnce(&Transaction<'_>) -> Result<(), rusqlite::Error> + Send + 'static,
    {
        let (by, at) = (by.to_string(), Utc::now().to_rfc3339());
        let old = h.old.map(|v| v.to_string());
        let new = h.new.map(|v| v.to_string());
        let (what, target) = (h.what, h.target);
        self.db
            .call(move |c| {
                let tx = c.transaction()?;
                f(&tx)?;
                tx.execute(
                    "INSERT INTO dex_wallet_history (what, target, old_value, new_value, changed_by, changed_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![what, target, old, new, by, at],
                )?;
                tx.commit()
            })
            .await?;
        Ok(())
    }

    /// Имя кошелька: без пробелов по краям, не пустое, не занято другим кошельком (без учёта регистра).
    /// Принимает: имя, id самого кошелька (при переименовании). Ошибки: Invalid.
    pub(crate) fn check_name(&self, name: &str, me: Option<&str>) -> Result<String, EditError> {
        let n = name.trim().to_string();
        if n.is_empty() {
            return Err(EditError::Invalid("Не указано имя кошелька".into()));
        }
        let low = n.to_lowercase();
        let taken = self
            .wallets()
            .into_iter()
            .any(|w| Some(w.id.as_str()) != me && w.name.to_lowercase() == low);
        if taken {
            return Err(EditError::Invalid(format!(
                "Имя «{n}» уже занято другим кошельком"
            )));
        }
        Ok(n)
    }

    /// Кошелёк по id или ошибка «нет такого». Ошибки: Invalid.
    pub(crate) fn must(&self, id: &str) -> Result<Wallet, EditError> {
        self.wallet(id)
            .ok_or_else(|| EditError::Invalid(format!("Кошелька {id} нет")))
    }

    /// Обновляет кошелёк в памяти. Ошибок нет.
    pub(crate) fn put(&self, w: Wallet) {
        self.st
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .wallets
            .insert(w.id.clone(), w);
    }

    /// Запись в журнал (вид wallets.changed; сбой журнала уже выведен в лог процесса). Ошибок нет.
    pub(crate) async fn say(&self, text: String) {
        let _ = self
            .journal
            .write(Level::Info, JOURNAL_KIND, None, text, None)
            .await;
        // Номер версии растёт; нет слушателей — не ошибка.
        self.changed.send_modify(|v| *v += 1);
    }
}
