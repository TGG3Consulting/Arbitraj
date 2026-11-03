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

/// Что переносить — задаёт main.rs из констант адаптеров (id биржи, id DEX, имя старого секрета), чтобы здесь не было
/// имён площадок.
#[derive(Debug, Clone)]
pub struct LegacyCfg {
    /// Имя кошелька переноса (решение Тиграна).
    pub name: String,
    /// Имя старого секрета ключа.
    pub secret: String,
    /// Ключ прежней строки очереди (id площадки DEX).
    pub queue_key: String,
    /// Старая настройка адреса исполнителя и её область (значение читается в момент переноса).
    pub executor_setting: String,
    pub executor_scope: crate::settings::scope::Scope,
    /// Строки маппинга кошелька переноса.
    pub rows: Vec<String>,
}

impl crate::settings::store::Store {
    /// Перенос по настройке main.rs (configure_legacy): есть ли старый ключ — говорит вызывающий (api/secrets.rs после
    /// разблокировки и после ввода секрета). Возвращает: кошелёк переноса (Some — перенос сделан сейчас).
    /// Ошибки: база. Не настроено — ничего.
    pub async fn migrate_wallets(
        &self,
        has_key: bool,
        by: &str,
    ) -> Result<Option<Wallet>, EditError> {
        let Some(cfg) = self.accounts().legacy_cfg() else {
            return Ok(None);
        };
        let executor = self
            .get_exact(&cfg.executor_setting, &cfg.executor_scope)
            .and_then(|v| v.as_str().map(str::to_string));
        let rows: Vec<&str> = cfg.rows.iter().map(String::as_str).collect();
        let l = Legacy {
            name: &cfg.name,
            secret: &cfg.secret,
            queue_key: &cfg.queue_key,
            executor,
            rows: &rows,
        };
        self.accounts().migrate(has_key, l, by).await
    }
}

impl Accounts {
    /// Задаёт, что переносить (main.rs при запуске). Ошибок нет.
    pub fn configure_legacy(&self, cfg: LegacyCfg) {
        *self.legacy.write().unwrap_or_else(|p| p.into_inner()) = Some(cfg);
    }

    /// Что переносить (None — не настроено). Ошибок нет.
    pub fn legacy_cfg(&self) -> Option<LegacyCfg> {
        self.legacy
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Имя старого секрета, наличие которого запускает перенос (None — не настроено). Ошибок нет.
    pub fn legacy_secret(&self) -> Option<String> {
        self.legacy_cfg().map(|c| c.secret)
    }

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
