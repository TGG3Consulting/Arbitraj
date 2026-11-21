//! Файл: core/coin_key.rs
//! Что делает: ключ монеты для замков и карантина (решение 118, Я-2, Я-18): адрес контракта — главный
//! идентификатор (одинаковый тикер на разных площадках бывает у разных токенов, решение 115); у монеты без адреса —
//! тикер (такие не торгуются, но ключ определён). Показ — тикер и короткий адрес. Решение 416: в ключе и сеть монеты
//! (Asset.chain) — одинаковый адрес в двух сетях — разные ключи; сделки одной монеты в разных сетях друг друга не ждут.
//! Кто вызывает: core/executor.rs, candidates.rs, ledger.rs, manual_halted.rs (замок «одна сделка на монету»),
//! core/risk.rs (карантин), core/reconcile.rs (вложено по монетам на «Балансах»).
//! Ключ монеты на счёте площадки (биржа — одна монета на все сети) — core/coin_account.rs.
//! Решение 487 (сети в id): строковый id монеты для записей учёта, интерфейса и области настроек «монета» — rid /
//! scope_id / parse_rid: у монеты другой сети (Asset.chain = Some) — суффикс «@сеть», у сети по умолчанию — прежний.
//! От чего зависит: core/types (Asset, ChainId), alloy-primitives.
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
    /// Решение 416: монета на счёте биржи — тикер этой биржи (остаток один на все сети; площадка — рядом в ключе
    /// замка). Даёт только core/coin_account.rs, когда сеть монеты указана и биржа её знает.
    Venue(String),
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
            CoinKey::Venue(t) => f.debug_tuple("Venue").field(t).finish(),
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

/// Решение 487 (сети в id): id монеты в записях учёта (core_state) и в интерфейсе — адрес контракта (вид «0x…» как
/// Address::to_string) или тикер; у монеты другой сети — «…@сеть». Сеть по умолчанию — id прежний (старые записи
/// читаются как есть). Ошибок нет.
pub fn rid(a: &Asset) -> String {
    let coin = a
        .contract
        .map(|x| x.to_string())
        .unwrap_or_else(|| a.ticker.clone());
    match &a.chain {
        Some(c) => format!("{coin}@{}", c.0),
        None => coin,
    }
}

/// Решение 487: id области настроек «монета» (settings/scope.rs coin_id_in): адрес строчными или тикер, у монеты
/// другой сети — «…@сеть». Ошибок нет.
pub fn scope_id(a: &Asset) -> String {
    let chain = a.chain.as_ref().map(|c| c.0.as_str());
    crate::settings::scope::coin_id_in(&a.ticker, a.contract.as_ref(), chain)
}

/// Решение 487: разбор id rid (из интерфейса): «id@сеть» → (id, Some(сеть)), без «@» — (id, None). Ошибок нет.
pub fn parse_rid(id: &str) -> (&str, Option<ChainId>) {
    match id.split_once('@') {
        Some((c, net)) if !net.is_empty() => (c, Some(ChainId(net.to_string()))),
        _ => (id, None),
    }
}
