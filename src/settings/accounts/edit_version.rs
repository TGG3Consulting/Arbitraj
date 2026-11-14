//! Файл: settings/accounts/edit_version.rs
//! Что делает: решение 272 — версия исполнителя ArbExecutor кошелька DEX («v1» / «v2») по коду в сети и (ревью 272, А1)
//! адрес обёртки нативной монеты из хранилища исполнителя v2. Пишет их только адаптер DEX после сверки кода по адресу
//! (exec/executor/onchain.rs): побайтно равен contracts/out/v1/… — «v1», равен contracts/out/… — «v2» (+ обёртка из
//! слота). Смена адреса исполнителя (edit.rs) их сбрасывает (None = не сверена = как v1). База (строка + история
//! «было/стало») → память → журнал wallets.changed (по нему экран перечитывает строку версии).
//! Ревью 272 (Б1): запись версии НЕ поднимает общий сигнал «кошельки изменились» (Accounts::changed) — он перезапускает
//! задачи событий всех кошельков (venues/bsc_dex/resubscribe.rs → resubscribe_wallets.rs), а сверка идёт сама при первой
//! сделке каждого кошелька, прямо во время торговли (ожидания зачислений в задачах wallet::events терялись бы). Адрес,
//! ключ и маппинг кошелька версия не меняет — задачам кошельков пересобираться не из-за чего.
//! Кто вызывает: venues/bsc_dex/exec/executor/onchain.rs, api/wallets_dex.rs (откат «Записать после проверки»).
//! От чего зависит: settings/accounts/{mod, edit, edit_map}.rs, storage/journal.
//! Настройки: нет.

use rusqlite::params;
use serde_json::json;

use crate::storage::journal::Level;

use super::edit::{EditError, Hist, JOURNAL_KIND};
use super::{Accounts, Wallet};

impl Accounts {
    /// Записывает версию исполнителя кошелька и адрес обёртки из его хранилища, если адрес исполнителя всё ещё тот, что
    /// сверяли (иначе — ничего: адрес сменили между сверкой и записью). Принимает: id кошелька, сверенный адрес
    /// исполнителя (0x…), версию, обёртку (0x…; None — не v2), кто. Возвращает: запись кошелька. Ошибки: Invalid (нет
    /// кошелька), база.
    pub async fn set_executor_version(
        &self,
        id: &str,
        executor: &str,
        version: Option<String>,
        wrapped: Option<String>,
        by: &str,
    ) -> Result<Wallet, EditError> {
        let _turn = self.writer.lock().await;
        let mut w = self.must(id)?;
        let same = w
            .executor
            .as_deref()
            .is_some_and(|x| x.eq_ignore_ascii_case(executor));
        if !same || (w.executor_version == version && w.executor_wrapped == wrapped) {
            return Ok(w);
        }
        let h = Hist {
            what: "wallet.executor_version",
            target: id.to_string(),
            old: Some(json!({"version": w.executor_version, "wrapped": w.executor_wrapped})),
            new: Some(json!({"version": version, "wrapped": wrapped})),
        };
        let (i, v, x) = (id.to_string(), version.clone(), wrapped.clone());
        self.commit(h, by, move |tx| {
            tx.execute(
                "UPDATE dex_wallets SET executor_version = ?2, executor_wrapped = ?3 WHERE id = ?1",
                params![i, v, x],
            )
            .map(|_| ())
        })
        .await?;
        let wrap = wrapped
            .as_deref()
            .map(|x| format!(", обёртка {x}"))
            .unwrap_or_default();
        let text = format!(
            "Кошелёк DEX «{}»: версия исполнителя {}{wrap} ({by})",
            w.name,
            version.as_deref().unwrap_or("не сверена")
        );
        w.executor_version = version;
        w.executor_wrapped = wrapped;
        self.put(w.clone());
        // Б1: только журнал (экран перечитает), без Accounts::changed — подписки кошельков не перезапускаются.
        let _ = self
            .journal
            .write(Level::Info, JOURNAL_KIND, None, text, None)
            .await;
        Ok(w)
    }
}
