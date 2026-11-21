//! Файл: core/types/account.rs
//! Что делает: абстрактный id счёта DEX (решение 132) — кошелька внутри одной площадки DEX. Ядро знает только id:
//! адреса и ключи — у адаптера DEX (по id он выбирает ключ, очередь транзакций и исполнитель). Сделка запоминает id
//! (core/deal.rs), шлюз отдаёт вид площадки, привязанный к счёту (gateway/registry.rs get_as).
//! Кто вызывает: core/dex_account.rs, core/deal.rs, gateway/contract.rs, gateway/registry.rs, адаптер DEX.
//! От чего зависит: serde.
//! Настройки: нет (кошельки и маппинг — settings/accounts/, экран «Маппинг кошельков»).

use serde::{Deserialize, Serialize};

/// id счёта (кошелька) DEX.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DexAccountId(pub String);

impl std::fmt::Display for DexAccountId {
    /// Печатает id как есть. Ошибок нет.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
