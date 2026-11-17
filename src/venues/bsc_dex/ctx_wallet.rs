//! Файл: venues/bsc_dex/ctx_wallet.rs
//! Что делает: кошелёк счёта DEX для окружения адаптера (решение 132): запись кошелька по id счёта (Ctx.account) или,
//! без счёта, — кошелёк строки «пробы» (только симуляции и чтение); обязательный счёт для подписи, отправки и всего,
//! что отдаёт адрес кошелька наружу; адрес исполнителя ArbExecutor кошелька; привязка окружения к счёту.
//! Ключ здесь не читается: только имя секрета (ключ — wallet/signer.rs, из зашифрованных секретов, только в памяти).
//! Кто вызывает: wallet/signer.rs, wallet/nonce.rs, wallet/queue_store.rs, exec/executor/mod.rs, pools/exec_kind.rs,
//! venue.rs, venue_account.rs, connection.rs.
//! От чего зависит: ctx.rs, settings/accounts/ (Store::accounts; снимок with_wallet — решение 401, ревью Н1),
//! exec/executor/code.rs (Version).
//! Настройки: нет (кошельки и маппинг — экран «Маппинг кошельков»).

use alloy_primitives::Address;

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::settings::accounts::{ROW_PROBE, Wallet};

use super::ctx::Ctx;
use super::exec::executor::code::Version;

/// Подсказка, где задать кошелёк.
pub const SCREEN: &str = "экран «Маппинг кошельков»";

impl Ctx {
    /// Окружение, привязанное к счёту. Принимает: id счёта. Возвращает: копию окружения. Ошибок нет (есть ли такой
    /// кошелёк — проверяет wallet()).
    pub fn with_account(&self, id: &str) -> Ctx {
        let mut c = self.clone();
        c.account = Some(id.to_string());
        c
    }

    /// Счёт обязателен: подписать, отправить транзакцию, отдать адрес кошелька наружу (депозит, балансы, очередь)
    /// можно только от имени счёта сделки. Возвращает: id счёта. Ошибки: NotConfigured «кошелёк не выбран».
    pub fn bound(&self) -> GatewayResult<&str> {
        self.account.as_deref().ok_or_else(|| {
            self.err(
                ErrorCode::NotConfigured,
                format!(
                    "Кошелёк DEX не выбран: действие с деньгами идёт только от кошелька сделки или биржи ({SCREEN})"
                ),
            )
        })
    }

    /// Кошелёк окружения: счёта (если привязан) или строки «пробы». Ошибки: NotConfigured — кошелька нет или строка
    /// «пробы» не задана.
    pub fn wallet(&self) -> GatewayResult<Wallet> {
        let acc = self.settings.accounts();
        let id = match &self.account {
            Some(id) => id.clone(),
            None => acc.row(ROW_PROBE).ok_or_else(|| {
                self.err(
                    ErrorCode::NotConfigured,
                    format!("Не задан кошелёк строки «Пробы и сигналы» ({SCREEN})"),
                )
            })?,
        };
        acc.wallet(&id).ok_or_else(|| {
            self.err(
                ErrorCode::NotConfigured,
                format!("Кошелька DEX {id} нет ({SCREEN})"),
            )
        })
    }

    /// Имя кошелька для журнала и ошибок (без адреса и ключа). Ошибок нет: нет кошелька — «кошелёк не выбран».
    pub fn wallet_name(&self) -> String {
        self.wallet()
            .map(|w| w.name)
            .unwrap_or_else(|_| "кошелёк не выбран".into())
    }

    /// Адрес исполнителя ArbExecutor кошелька окружения. Возвращает: None — не развёрнут (или кошелька нет).
    /// Ошибки: Internal — адрес записан не адресом. Решение 401 (ревью t186, Н1): из снимка кошельков без замка
    /// (Accounts::with_wallet), без копии записи кошелька.
    pub fn executor(&self) -> GatewayResult<Option<Address>> {
        let acc = self.settings.accounts();
        acc.with_wallet(self.account.as_deref(), |found| {
            let Some((w, x)) = found.and_then(|r| r.executor.as_deref().map(|a| (r, a))) else {
                return Ok(None);
            };
            x.parse::<Address>().map(Some).map_err(|_| {
                self.err(
                    ErrorCode::Internal,
                    format!("Кошелёк «{}»: исполнитель {x} — не адрес", w.name),
                )
            })
        })
    }

    /// Решение 401 (ревью t186, Н1): горячий путь котировки — адрес исполнителя кошелька окружения и его сверенная
    /// версия одним чтением снимка кошельков без замка (Accounts::with_wallet), без копий и строк. Возвращает: None —
    /// кошелька или исполнителя нет, или адрес не разбирается; Some((адрес, None)) — версия не сверена. Ошибок нет.
    pub fn exec_info(&self) -> Option<(Address, Option<Version>)> {
        let acc = self.settings.accounts();
        acc.with_wallet(self.account.as_deref(), |found| {
            let w = found?;
            let x = w.executor.as_deref()?.parse::<Address>().ok()?;
            Some((x, w.executor_version.as_deref().and_then(Version::parse)))
        })
    }
}
