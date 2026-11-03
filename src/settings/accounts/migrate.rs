//! Файл: settings/accounts/migrate.rs
//! Что делает: перенос старого единственного кошелька DEX (решение 132, пункт 5) — один раз: если кошельков ещё нет и
//! старый ключ есть в секретах, создаётся кошелёк (имя — из решения Тиграна) с ТЕМ ЖЕ секретом (ключ не переписывается
//! и не копируется), исполнитель — текущее значение старой настройки, очередь — прежняя строка wallet_queue, строки
//! маппинга (биржа переноса и «пробы») → этот кошелёк. Отметка «перенос выполнен» — в dex_wallet_meta; кошельки уже
//! есть — отметка без переноса. Ключа нет — ничего не отмечается (перенос повторится при следующей разблокировке).
//! Деньги никуда не переводятся: это только запись «счёт = прежний кошелёк».
//! Кто вызывает: api/secrets.rs (после разблокировки и после ввода секрета) — через main-уровень параметров
//! (api/wallets_migrate.rs).
//! От чего зависит: settings/accounts/{mod,db,edit,edit_map}.rs.
//! Настройки: нет (значение старой настройки передаёт вызывающий).

use chrono::Utc;
use rusqlite::params;

use super::db::{META_LEGACY, META_MIGRATED};
use super::edit::EditError;
use super::{Accounts, Wallet};

/// Что переносится.
pub struct Legacy<'a> {
    /// Имя кошелька переноса.
    pub name: &'a str,
    /// Имя старого секрета ключа.
    pub secret: &'a str,
    /// Ключ прежней строки очереди транзакций (storage/wallet_queue.rs).
    pub queue_key: &'a str,
    /// Адрес исполнителя из старой настройки (если был задан).
    pub executor: Option<String>,
    /// Строки маппинга, которые ведёт кошелёк переноса.
    pub rows: &'a [&'a str],
}

impl Accounts {
    /// Выполняет перенос, если он нужен. Принимает: есть ли старый ключ в секретах, что переносить, кто.
    /// Возвращает: кошелёк переноса (Some — перенос сделан сейчас). Ошибки: база.
    pub async fn migrate(
        &self,
        has_key: bool,
        l: Legacy<'_>,
        by: &str,
    ) -> Result<Option<Wallet>, EditError> {
        let _turn = self.writer.lock().await;
        if self.read().migrated {
            return Ok(None);
        }
        if !self.read().wallets.is_empty() {
            self.mark(None).await?;
            return Ok(None);
        }
        if !has_key {
            return Ok(None);
        }
        let id = self.reserve_locked().await?;
        let w = Wallet {
            id: id.clone(),
            name: l.name.to_string(),
            secret: l.secret.to_string(),
            executor: None,
            queue_key: l.queue_key.to_string(),
            created_at: Utc::now().to_rfc3339(),
        };
        self.insert_locked(w, by).await?;
        if l.executor.is_some() {
            self.executor_locked(&id, l.executor.clone(), by).await?;
        }
        for row in l.rows {
            self.map_locked(row, Some(&id), by).await?;
        }
        self.mark(Some(&id)).await?;
        let text = format!(
            "Перенос кошелька DEX (решение 132): прежний ключ {} — кошелёк «{}», исполнитель {}, маппинг: {}. \
             Деньги не переводились",
            l.secret,
            l.name,
            l.executor.as_deref().unwrap_or("не задан"),
            l.rows.join(", ")
        );
        self.say(text).await;
        Ok(self.wallet(&id))
    }

    /// Отметка «перенос выполнен» (и id кошелька переноса). Ошибки: база.
    async fn mark(&self, legacy: Option<&str>) -> Result<(), EditError> {
        let l = legacy.map(str::to_string);
        self.db
            .call(move |c| {
                let tx = c.transaction()?;
                let at = Utc::now().to_rfc3339();
                tx.execute(
                    "INSERT OR REPLACE INTO dex_wallet_meta (key, value) VALUES (?1, ?2)",
                    params![META_MIGRATED, at],
                )?;
                if let Some(l) = &l {
                    tx.execute(
                        "INSERT OR REPLACE INTO dex_wallet_meta (key, value) VALUES (?1, ?2)",
                        params![META_LEGACY, l],
                    )?;
                }
                tx.commit()
            })
            .await?;
        let mut st = self.st.write().unwrap_or_else(|p| p.into_inner());
        st.migrated = true;
        if legacy.is_some() {
            st.legacy = legacy.map(str::to_string);
        }
        Ok(())
    }
}
