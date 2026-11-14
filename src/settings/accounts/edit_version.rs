//! Файл: settings/accounts/edit_version.rs
//! Что делает: решение 272 — версия исполнителя ArbExecutor кошелька DEX («v1» / «v2») по коду в сети. Пишет её только
//! адаптер DEX после сверки кода по адресу (exec/executor/onchain.rs): побайтно равен contracts/out/v1/… — «v1», равен
//! contracts/out/… — «v2». Смена адреса исполнителя (edit.rs) версию сбрасывает (None = не сверена = как v1). Как и
//! прочие изменения кошельков: база (строка + история «было/стало») → память → журнал wallets.changed.
//! Кто вызывает: venues/bsc_dex/exec/executor/onchain.rs.
//! От чего зависит: settings/accounts/{mod, edit, edit_map}.rs.
//! Настройки: нет.

use rusqlite::params;
use serde_json::json;

use super::edit::{EditError, Hist};
use super::{Accounts, Wallet};

impl Accounts {
    /// Записывает версию исполнителя кошелька, если адрес исполнителя всё ещё тот, что сверяли (иначе — ничего: адрес
    /// сменили между сверкой и записью). Принимает: id кошелька, сверенный адрес исполнителя (0x…), версию, кто.
    /// Возвращает: запись кошелька. Ошибки: Invalid (нет кошелька), база.
    pub async fn set_executor_version(
        &self,
        id: &str,
        executor: &str,
        version: Option<String>,
        by: &str,
    ) -> Result<Wallet, EditError> {
        let _turn = self.writer.lock().await;
        let mut w = self.must(id)?;
        let same = w
            .executor
            .as_deref()
            .is_some_and(|x| x.eq_ignore_ascii_case(executor));
        if !same || w.executor_version == version {
            return Ok(w);
        }
        let h = Hist {
            what: "wallet.executor_version",
            target: id.to_string(),
            old: w.executor_version.as_ref().map(|x| json!(x)),
            new: version.as_ref().map(|x| json!(x)),
        };
        let (i, v) = (id.to_string(), version.clone());
        self.commit(h, by, move |tx| {
            tx.execute(
                "UPDATE dex_wallets SET executor_version = ?2 WHERE id = ?1",
                params![i, v],
            )
            .map(|_| ())
        })
        .await?;
        let text = format!(
            "Кошелёк DEX «{}»: версия исполнителя {} ({by})",
            w.name,
            version.as_deref().unwrap_or("не сверена")
        );
        w.executor_version = version;
        self.put(w.clone());
        self.say(text).await;
        Ok(w)
    }
}
