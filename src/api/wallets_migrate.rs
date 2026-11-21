//! Файл: api/wallets_migrate.rs
//! Что делает: перенос прежнего единственного кошелька DEX (решение 132, пункт 5) после разблокировки секретов и после
//! ввода секрета: старый ключ есть в секретах и кошельков ещё нет — кошелёк переноса и строки маппинга
//! (settings/accounts/migrate.rs, что переносить — main.rs). Итог — в журнал (wallets.changed) и уведомление
//! wallet.added. Деньги не переводятся; ключ не читается — проверяется только имя секрета.
//! Кто вызывает: api/secrets.rs (unlock, put).
//! От чего зависит: api/state.rs, settings/accounts/, secrets (имена), notify.
//! Настройки: нет.

use crate::storage::journal::Level;

use super::state::AppState;

/// Выполняет перенос, если он нужен. Ошибок нет: сбой — в журнал.
pub async fn after_unlock(st: &AppState) {
    let has = st
        .settings
        .accounts()
        .legacy_secret()
        .is_some_and(|s| st.vault.status().names.contains(&s));
    match st
        .settings
        .migrate_wallets(has, "перенос (решение 132)")
        .await
    {
        Ok(Some(w)) => {
            let text = format!(
                "Кошелёк DEX «{}» создан переносом прежнего ключа (решение 132); маппинг: {}",
                w.name,
                st.settings.accounts().rows_of(&w.id).join(", ")
            );
            st.notifier.send("wallet.added", None, &text).await;
        }
        Ok(None) => {}
        Err(e) => {
            let text = format!("Перенос кошелька DEX (решение 132) не выполнен: {e}");
            let _ = st
                .journal
                .write(Level::Error, "wallets.changed", None, text, None)
                .await;
        }
    }
}
