//! Файл: core/coin_key.rs
//! Что делает: ключ монеты для замков и карантина (решение 118, Я-2, Я-18): адрес контракта в BSC — главный
//! идентификатор (одинаковый тикер на разных площадках бывает у разных токенов, решение 115); у монеты без адреса —
//! тикер (такие не торгуются, но ключ определён). Показ — тикер и короткий адрес.
//! Кто вызывает: core/executor.rs, candidates.rs, ledger.rs, manual_halted.rs (замок «одна сделка на монету»),
//! core/risk.rs (карантин), core/reconcile.rs (вложено по монетам на «Балансах»).
//! От чего зависит: core/types (Asset), alloy-primitives.
//! Настройки: нет.

use alloy_primitives::Address;
use serde::{Deserialize, Serialize};

use crate::core::types::Asset;

/// Ключ монеты: адрес контракта или (у монеты без адреса) тикер.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CoinKey {
    /// Адрес контракта токена в BSC.
    Contract(Address),
    /// Тикер монеты без адреса контракта.
    Ticker(String),
}

impl CoinKey {
    /// Ключ монеты. Принимает: монету. Возвращает: адрес контракта или тикер. Ошибок нет.
    pub fn of(a: &Asset) -> Self {
        match a.bsc_contract {
            Some(c) => CoinKey::Contract(c),
            None => CoinKey::Ticker(a.ticker.clone()),
        }
    }
}

/// Короткая подпись адреса для показа: первые 6 и последние 4 знака («0x1234…abcd»). Ошибок нет.
pub fn short(a: &Address) -> String {
    let s = a.to_string();
    match (s.get(..6), s.get(s.len().saturating_sub(4)..)) {
        (Some(h), Some(t)) if s.len() > 10 => format!("{h}…{t}"),
        _ => s,
    }
}

/// Подпись монеты для показа: «тикер (0x1234…abcd)», у монеты без адреса — тикер. Ошибок нет.
pub fn label(a: &Asset) -> String {
    match &a.bsc_contract {
        Some(c) => format!("{} ({})", a.ticker, short(c)),
        None => a.ticker.clone(),
    }
}
