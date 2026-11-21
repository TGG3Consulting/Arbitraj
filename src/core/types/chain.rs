//! Файл: core/types/chain.rs
//! Что делает: решение 416 (шаг 1 плана двух сетей, Тигран 19.11.2026) — тип «сеть» ядра: строковый id сети. Значение
//! берётся только из настроек (dex.chain площадки DEX, например «bsc» или «arbitrum»); ядро конкретных сетей не знает и
//! сравнивает id только на равенство.
//! Кто вызывает: core/types/market.rs (Asset.chain), core/coin_key.rs (ключ монеты с сетью), core/chain.rs (сеть площадки
//! DEX из настроек), адаптер DEX.
//! От чего зависит: serde.
//! Настройки: нет (значение — dex.chain, читает core/chain.rs).

use serde::{Deserialize, Serialize};

/// Id сети (например «bsc»). Пустая строка не бывает: настройка dex.chain — текст, пустой не задаётся.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChainId(pub String);

impl ChainId {
    /// Что делает: id сети строкой (для подписи, журнала, ключа настроек). Ошибок нет.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ChainId {
    /// Что делает: показ id сети как есть. Ошибок нет.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
