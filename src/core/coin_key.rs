//! Файл: core/coin_key.rs
//! Что делает: ключ монеты для замков и карантина (решение 118, Я-2, Я-18): адрес контракта — главный
//! идентификатор (одинаковый тикер на разных площадках бывает у разных токенов, решение 115); у монеты без адреса —
//! тикер (такие не торгуются, но ключ определён). Показ — тикер и короткий адрес. Решение 416: в ключе и сеть монеты
//! (Asset.chain) — одинаковый адрес в двух сетях — разные ключи; сделки одной монеты в разных сетях друг друга не ждут.
//! Кто вызывает: core/executor.rs, candidates.rs, ledger.rs, manual_halted.rs (замок «одна сделка на монету»),
//! core/risk.rs (карантин), core/reconcile.rs (вложено по монетам на «Балансах»).
//! От чего зависит: core/types (Asset), alloy-primitives.
//! Настройки: нет.

use alloy_primitives::Address;
use serde::{Deserialize, Serialize};

use crate::core::types::{Asset, ChainId};

/// Ключ монеты: (сеть, адрес контракта) или, у монеты без адреса, (сеть, тикер). Решение 416: сеть — Asset.chain
/// (None — не указана; в шаге 1 у всех монет None — ключи как раньше).
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CoinKey {
    /// Сеть и адрес контракта токена в ней.
    Contract(Option<ChainId>, Address),
    /// Сеть и тикер монеты без адреса контракта.
    Ticker(Option<ChainId>, String),
}

impl CoinKey {
    /// Ключ монеты. Принимает: монету. Возвращает: (сеть, адрес контракта) или (сеть, тикер). Ошибок нет.
    pub fn of(a: &Asset) -> Self {
        match a.contract {
            Some(c) => CoinKey::Contract(a.chain.clone(), c),
            None => CoinKey::Ticker(a.chain.clone(), a.ticker.clone()),
        }
    }
}

impl std::fmt::Debug for CoinKey {
    /// Решение 416: вид для журнала прежний («Contract(0x…)», «Ticker("…")»); сеть — только если указана.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoinKey::Contract(None, a) => f.debug_tuple("Contract").field(a).finish(),
            CoinKey::Contract(Some(c), a) => f.debug_tuple("Contract").field(c).field(a).finish(),
            CoinKey::Ticker(None, t) => f.debug_tuple("Ticker").field(t).finish(),
            CoinKey::Ticker(Some(c), t) => f.debug_tuple("Ticker").field(c).field(t).finish(),
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
    match &a.contract {
        Some(c) => format!("{} ({})", a.ticker, short(c)),
        None => a.ticker.clone(),
    }
}
