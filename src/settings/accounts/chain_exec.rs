//! Файл: settings/accounts/chain_exec.rs
//! Что делает: решение 432 (шаг 4 двух сетей, Тигран 19.11.2026: кошельки одни и те же на обе сети) — кошелёк глазами
//! площадки DEX своей сети. Ключ и адрес у кошелька один на все сети, а по сети своё: исполнитель ArbExecutor
//! (развёртывается в каждой сети отдельно — свой адрес, версия, обёртка) и очередь транзакций (nonce у каждой сети свой).
//! **Домашняя площадка** кошелька — та, где он заведён: начало ключа очереди (`queue_key` «bsc_dex» у кошелька переноса,
//! «bsc_dex:w2» у добавленных — api/wallets.rs, main.rs). Её исполнитель — прежние поля Wallet.executor* (столбцы
//! dex_wallets), очередь — прежний queue_key: **у BSC ничего не меняется и не переносится**. На другой площадке DEX —
//! исполнитель из таблицы dex_wallet_exec (Wallet.chains; нет строки — не развёрнут), ключ очереди «<площадка>:<id>».
//! Кто вызывает: адаптер DEX (venues/bsc_dex/ctx_wallet.rs, pools/exec_callback.rs, exec/executor/onchain.rs, проходы
//! пулов), core/executor_deploy.rs, api/wallets_dex.rs, api/wallets_view.rs, тесты tests/accounts_chain_exec_432.rs.
//! От чего зависит: settings/accounts/{mod, edit, edit_map, edit_version}.rs, storage/db.rs (таблица dex_wallet_exec).
//! Настройки: нет (кошельки — экран «Маппинг кошельков»).

use chrono::Utc;
use rusqlite::params;
use serde::Serialize;
use serde_json::json;

use crate::storage::journal::Level;

use super::edit::{EditError, Hist, JOURNAL_KIND};
use super::{Accounts, Wallet};

/// Исполнитель кошелька на площадке DEX другой сети (строка dex_wallet_exec). Поля — как Wallet.executor*.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ExecAt {
    pub executor: Option<String>,
    pub executor_version: Option<String>,
    pub executor_wrapped: Option<String>,
}

impl Wallet {
    /// Что делает: домашняя площадка DEX кошелька — начало ключа очереди до «:». Ошибок нет.
    pub fn home(&self) -> &str {
        self.queue_key.split(':').next().unwrap_or(&self.queue_key)
    }

    /// Что делает: запись кошелька глазами площадки DEX `venue`: домашняя — как есть; другая — исполнитель этой площадки
    /// (нет — None) и ключ очереди «<площадка>:<id>». Принимает: id площадки. Ошибок нет.
    pub fn at(&self, venue: &str) -> Wallet {
        if self.home() == venue {
            return self.clone();
        }
        let x = self.chains.get(venue).cloned().unwrap_or_default();
        Wallet {
            executor: x.executor,
            executor_version: x.executor_version,
            executor_wrapped: x.executor_wrapped,
            queue_key: format!("{venue}:{}", self.id),
            ..self.clone()
        }
    }
}

impl Accounts {
    /// Что делает: with_wallet глазами площадки `venue` (решение 401: домашняя площадка — из снимка без копии; другая —
    /// копия записи с исполнителем этой площадки). Принимает: площадку, id счёта (None — строка «Пробы»), что сделать.
    /// Возвращает: итог `f`. Ошибок нет.
    pub fn with_wallet_at<R>(
        &self,
        venue: &str,
        account: Option<&str>,
        f: impl FnOnce(Option<&Wallet>) -> R,
    ) -> R {
        self.with_wallet(account, |found| match found {
            Some(w) if w.home() != venue => f(Some(&w.at(venue))),
            other => f(other),
        })
    }

    /// Что делает: кошелёк по id глазами площадки `venue` (Wallet::at). None — нет кошелька. Ошибок нет.
    pub fn wallet_at(&self, venue: &str, id: &str) -> Option<Wallet> {
        self.wallet(id).map(|w| w.at(venue))
    }

    /// Что делает: все кошельки глазами площадки `venue`. Ошибок нет.
    pub fn wallets_at(&self, venue: &str) -> Vec<Wallet> {
        self.wallets().into_iter().map(|w| w.at(venue)).collect()
    }

    /// Что делает: задаёт или снимает адрес исполнителя кошелька на площадке `venue`: домашняя — set_executor (как
    /// раньше); другая — строка dex_wallet_exec (версия и обёртка сбрасываются — не сверены). Принимает: площадку, id,
    /// адрес (0x… или None), кто. Возвращает: запись кошелька глазами площадки. Ошибки: Invalid (нет кошелька), база.
    pub async fn set_executor_at(
        &self,
        venue: &str,
        id: &str,
        executor: Option<String>,
        by: &str,
    ) -> Result<Wallet, EditError> {
        let w0 = self.must(id)?;
        if w0.home() == venue {
            return self.set_executor(id, executor, by).await;
        }
        let _turn = self.writer.lock().await;
        let mut w = self.must(id)?;
        let old = w.chains.get(venue).and_then(|x| x.executor.clone());
        if old == executor {
            return Ok(w.at(venue));
        }
        let h = Hist {
            what: "wallet.executor",
            target: format!("{id}@{venue}"),
            old: old.as_ref().map(|x| json!(x)),
            new: executor.as_ref().map(|x| json!(x)),
        };
        let (i, v, x) = (id.to_string(), venue.to_string(), executor.clone());
        self.commit(h, by, move |tx| {
            match x {
                Some(x) => tx.execute(
                    "INSERT INTO dex_wallet_exec (wallet, venue, executor, executor_version, executor_wrapped, \
                     updated_at) VALUES (?1, ?2, ?3, NULL, NULL, ?4) ON CONFLICT(wallet, venue) DO UPDATE SET \
                     executor = ?3, executor_version = NULL, executor_wrapped = NULL, updated_at = ?4",
                    params![i, v, x, Utc::now().to_rfc3339()],
                ),
                None => tx.execute(
                    "DELETE FROM dex_wallet_exec WHERE wallet = ?1 AND venue = ?2",
                    params![i, v],
                ),
            }
            .map(|_| ())
        })
        .await?;
        let shown = executor.clone().unwrap_or_else(|| "не задан".into());
        let text = format!("Кошелёк DEX «{}» на {venue}: исполнитель {shown} ({by})", w.name);
        match executor {
            Some(x) => {
                w.chains.insert(
                    venue.to_string(),
                    ExecAt {
                        executor: Some(x),
                        ..ExecAt::default()
                    },
                );
            }
            None => {
                w.chains.remove(venue);
            }
        }
        self.put(w.clone());
        self.say(text).await;
        Ok(w.at(venue))
    }

    /// Что делает: версия исполнителя и обёртка кошелька на площадке `venue` после сверки кода (решение 272) — если
    /// адрес исполнителя всё ещё тот, что сверяли. Домашняя площадка — set_executor_version (как раньше). Общий сигнал
    /// «кошельки изменились» не поднимается (ревью 272, Б1). Принимает: площадку, id, сверенный адрес, версию, обёртку,
    /// кто. Возвращает: запись глазами площадки. Ошибки: Invalid (нет кошелька), база.
    pub async fn set_executor_version_at(
        &self,
        venue: &str,
        id: &str,
        executor: &str,
        version: Option<String>,
        wrapped: Option<String>,
        by: &str,
    ) -> Result<Wallet, EditError> {
        let w0 = self.must(id)?;
        if w0.home() == venue {
            return self.set_executor_version(id, executor, version, wrapped, by).await;
        }
        let _turn = self.writer.lock().await;
        let mut w = self.must(id)?;
        let cur = w.chains.get(venue).cloned().unwrap_or_default();
        let same = cur
            .executor
            .as_deref()
            .is_some_and(|x| x.eq_ignore_ascii_case(executor));
        if !same || (cur.executor_version == version && cur.executor_wrapped == wrapped) {
            return Ok(w.at(venue));
        }
        let h = Hist {
            what: "wallet.executor_version",
            target: format!("{id}@{venue}"),
            old: Some(json!({"version": cur.executor_version, "wrapped": cur.executor_wrapped})),
            new: Some(json!({"version": version, "wrapped": wrapped})),
        };
        let (i, v, ver, x) = (id.to_string(), venue.to_string(), version.clone(), wrapped.clone());
        self.commit(h, by, move |tx| {
            tx.execute(
                "UPDATE dex_wallet_exec SET executor_version = ?3, executor_wrapped = ?4, updated_at = ?5 \
                 WHERE wallet = ?1 AND venue = ?2",
                params![i, v, ver, x, Utc::now().to_rfc3339()],
            )
            .map(|_| ())
        })
        .await?;
        let text = format!(
            "Кошелёк DEX «{}» на {venue}: версия исполнителя {}",
            w.name,
            version.as_deref().unwrap_or("не сверена")
        );
        w.chains.insert(
            venue.to_string(),
            ExecAt {
                executor: cur.executor,
                executor_version: version,
                executor_wrapped: wrapped,
            },
        );
        self.put(w.clone());
        // Б1: только журнал, без Accounts::changed — подписки кошельков не перезапускаются.
        let _ = self
            .journal
            .write(Level::Info, JOURNAL_KIND, None, format!("{text} ({by})"), None)
            .await;
        Ok(w.at(venue))
    }
}
