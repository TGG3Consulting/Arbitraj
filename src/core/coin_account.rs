//! Файл: core/coin_account.rs
//! Что делает: решение 416 (требование Тиграна 19.11.2026) — ключ монеты НА СЧЁТЕ ПЛОЩАДКИ для учёта факта и очереди
//! продажи: замок счёта credit_lock.rs (решение 413), очередь продажи sell_turn.rs (решение 415), монеты соседей по
//! остатку remain_others.rs, разбор остатка credit_rest_park.rs, «монета в сделке» кнопки продажи balance_sell_check.rs.
//! На бирже остаток монеты ОДИН на все сети (баланс по тикеру биржи): сделки одной монеты биржи через BSC и через
//! Arbitrum — соседи по одному остатку (одна очередь продажи, один замок факта, правильная доля каждой). На DEX сеть
//! разделяет полностью (разные пулы, разные балансы кошелька): ключ — (сеть, адрес), CoinKey::of.
//! Правило ключа: у монеты указаны сеть и адрес, и биржа сообщила свой тикер этой монеты в этой сети (remember) —
//! CoinKey::Venue(тикер биржи); иначе — CoinKey::of (сеть, адрес). В шаге 1 сеть у монет не указана (None) — ключ
//! всегда CoinKey::of, как раньше (поведение побайтно прежнее). Карта «(биржа, сеть, адрес) → тикер биржи» — только в
//! памяти, только пополняется (тикер одной монеты биржи не «пропадает» посреди сделки — ключ замка устойчив);
//! заполняет её справочник монет бирж, когда биржи отдают адрес по каждой сети (шаг 3 плана, docs/TASKS.md).
//! Кто вызывает: core/credit_lock.rs, core/sell_turn.rs, core/remain_others.rs, core/credit_rest_park.rs,
//! core/balance_sell_check.rs; remember — справочник (шаг 3) и тесты.
//! От чего зависит: coin_key.rs, core/types.
//! Настройки: нет (базовое поведение).

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use alloy_primitives::Address;

use crate::core::types::{Asset, ChainId, VenueId};

use super::coin_key::CoinKey;

/// (биржа, сеть, адрес) → тикер этой биржи.
type Map = HashMap<(VenueId, ChainId, Address), String>;

static VENUE_COIN: LazyLock<RwLock<Map>> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// Что делает: запоминает тикер биржи для монеты (сеть, адрес) — монеты разных сетей с одним тикером биржи становятся
/// одной монетой на её счёте. Только для бирж (у DEX сеть разделяет счёт). Принимает: биржу, сеть, адрес, тикер биржи.
/// Ошибок нет.
pub fn remember(v: &VenueId, chain: &ChainId, addr: Address, ticker: &str) {
    VENUE_COIN
        .write()
        .unwrap_or_else(|p| p.into_inner())
        .insert((v.clone(), chain.clone(), addr), ticker.to_string());
}

/// Что делает: ключ монеты на счёте площадки (шапка). Принимает: площадку, монету. Возвращает: ключ. Ошибок нет.
pub fn key(v: &VenueId, a: &Asset) -> CoinKey {
    if let (Some(c), Some(addr)) = (&a.chain, a.contract)
        && let Some(t) =
            VENUE_COIN
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .get(&(v.clone(), c.clone(), addr))
    {
        return CoinKey::Venue(t.clone());
    }
    CoinKey::of(a)
}
