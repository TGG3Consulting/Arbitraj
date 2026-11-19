//! Файл: core/coin_account.rs
//! Что делает: решение 416 (требование Тиграна 19.11.2026) — ключ монеты НА СЧЁТЕ ПЛОЩАДКИ для учёта факта и очереди
//! продажи: замок счёта credit_lock.rs (решение 413), очередь продажи sell_turn.rs (решение 415), монеты соседей по
//! остатку remain_others.rs, разбор остатка credit_rest_park.rs, «монета в сделке» кнопки продажи balance_sell_check.rs.
//! На бирже остаток монеты ОДИН на все сети (баланс по тикеру биржи): сделки одной монеты биржи через BSC и через
//! Arbitrum — соседи по одному остатку (одна очередь продажи, один замок факта, правильная доля каждой). На DEX сеть
//! разделяет полностью (разные пулы, разные балансы кошелька): ключ — (сеть, адрес), CoinKey::of.
//! Правило ключа: сеть монеты — своя, а у монеты без сети (запись до решения 416, монета биржи) — сеть площадок DEX при
//! запуске ядра (dex.chain, core/chain.rs; запоминается один раз на процесс). Есть сеть и адрес, и биржа сообщила свой
//! тикер этой монеты в этой сети (remember) — CoinKey::Venue(тикер биржи); иначе — CoinKey::of.
//! Ревью шага 1 (С-3) — ключ не меняется посреди жизни сделки: карта «(биржа, сеть, адрес) → тикер биржи» только
//! пополняется (запись не перезаписывается и не удаляется), пишется в учёт ядра (core_state, вид coin_account) и
//! читается при запуске ДО восстановления сделок; сеть по умолчанию — одна на процесс. Значит, ключ — функция
//! неизменных за процесс данных; единственный переход «записи нет → есть» случается, когда справочник впервые видит
//! тикер биржи для (сеть, адрес), — до этого сделки этой монеты на этой бирже нет (сделка строится по присутствию
//! монеты на бирже в справочнике). Взятие и снятие замка — одна и та же охрана (Arc замка), ключ при снятии не
//! пересчитывается. В шаге 1 карту никто не пополняет (биржи отдают адрес только сети venue.bsc_network) — ключ всегда
//! CoinKey::of, поведение побайтно прежнее. Пополнять — справочник в шаге 3 (remember_saved до публикации справочника).
//! Кто вызывает: core/credit_lock.rs, core/sell_turn.rs, core/remain_others.rs, core/credit_rest_park.rs,
//! core/balance_sell_check.rs; load — core/engine.rs (запуск); remember — справочник (шаг 3) и тесты.
//! От чего зависит: coin_key.rs, chain.rs, persist.rs, core/types, engine.rs.
//! Настройки: dex.chain (через chain.rs, при запуске).

use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock, RwLock};

use alloy_primitives::Address;
use serde::{Deserialize, Serialize};

use crate::core::types::{Asset, ChainId, VenueId};
use crate::gateway::contract::VenueKind;
use crate::storage::journal::Level;

use super::coin_key::CoinKey;
use super::engine::{Core, rd};

/// Вид записи учёта ядра (core_state).
pub const KIND: &str = "coin_account";

/// Запись карты: биржа, сеть, адрес → тикер биржи.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub venue: VenueId,
    pub chain: ChainId,
    pub address: Address,
    pub ticker: String,
}

/// (биржа, сеть, адрес) → тикер этой биржи.
type Map = HashMap<(VenueId, ChainId, Address), String>;

static VENUE_COIN: LazyLock<RwLock<Map>> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// Сеть монет без сети — сеть площадок DEX при запуске ядра (первое успешное значение за процесс).
static DEFAULT: OnceLock<ChainId> = OnceLock::new();

/// Что делает: запоминает тикер биржи для монеты (сеть, адрес) — монеты разных сетей с одним тикером биржи становятся
/// одной монетой на её счёте. Запись не перезаписывается (С-3). Только для бирж. Принимает: биржу, сеть, адрес, тикер.
/// Возвращает: true — запись новая. Ошибок нет.
pub fn remember(v: &VenueId, chain: &ChainId, addr: Address, ticker: &str) -> bool {
    let mut m = VENUE_COIN.write().unwrap_or_else(|p| p.into_inner());
    match m.entry((v.clone(), chain.clone(), addr)) {
        std::collections::hash_map::Entry::Occupied(_) => false,
        std::collections::hash_map::Entry::Vacant(e) => {
            e.insert(ticker.to_string());
            true
        }
    }
}

/// Что делает: remember и, если запись новая, — в учёт ядра (переживает перезапуск). Принимает: ядро, запись.
/// Ошибок нет: сбой базы — журнал core.coin_account (ключ в памяти уже есть).
pub async fn remember_saved(core: &Core, e: &Entry) {
    if !remember(&e.venue, &e.chain, e.address, &e.ticker) {
        return;
    }
    let id = format!("{}:{}:{:#x}", e.venue.0, e.chain, e.address);
    if let Err(err) = super::persist::save(&core.inner.deps.db, KIND, &id, e).await {
        let m = format!("Монета биржи {id} → {} не записана в учёт: {err}", e.ticker);
        core.inner
            .deps
            .log(Level::Error, "core.coin_account", Some(&e.venue.0), m, None)
            .await;
    }
}

/// Что делает: при запуске ядра — карта из учёта и сеть монет без сети (dex.chain площадок DEX; не задана или сети
/// разные — монеты без сети ищутся без сети, CoinKey::of). Принимает: ядро. Ошибок нет: сбой базы — журнал.
pub async fn load(core: &Core) {
    match super::persist::load::<Entry>(&core.inner.deps.db, KIND).await {
        Ok(list) => {
            for e in list {
                remember(&e.venue, &e.chain, e.address, &e.ticker);
            }
        }
        Err(err) => {
            let m = format!("Монеты бирж по сетям (решение 416) не загружены: {err}");
            core.inner
                .deps
                .log(Level::Error, "core.coin_account", None, m, None)
                .await;
        }
    }
    let dex: Vec<VenueId> = rd(&core.inner.kinds)
        .iter()
        .filter(|(_, k)| *k == VenueKind::Dex)
        .map(|(v, _)| v.clone())
        .collect();
    if let Ok(c) = super::chain::dex_chain(&core.inner.deps.settings, &dex) {
        let _ = DEFAULT.set(c);
    }
}

/// Что делает: ключ монеты на счёте площадки (шапка). Принимает: площадку, монету. Возвращает: ключ. Ошибок нет.
pub fn key(v: &VenueId, a: &Asset) -> CoinKey {
    key_with(v, a, DEFAULT.get())
}

/// Что делает: ключ с явной сетью по умолчанию (для монеты без сети). Принимает: площадку, монету, сеть по умолчанию.
/// Возвращает: CoinKey::Venue(тикер биржи) — карта знает (площадка, сеть, адрес); иначе CoinKey::of. Ошибок нет.
pub fn key_with(v: &VenueId, a: &Asset, default: Option<&ChainId>) -> CoinKey {
    if let (Some(c), Some(addr)) = (a.chain.as_ref().or(default), a.contract)
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
