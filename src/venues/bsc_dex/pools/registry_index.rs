//! Файл: venues/bsc_dex/pools/registry_index.rs
//! Что делает: содержимое реестра пулов (pools/registry.rs) — записи за `Arc` и индексы «пара (монета, встречный
//! токен) → пулы» и «монета (адрес) → пулы» (решение 211: раньше «тикер монеты → пулы»; монета — адрес контракта,
//! тикер — только подпись). Решение 129 (голодание расчётов): раньше by_pair / by_ticker / pair_marks
//! проходили ВЕСЬ реестр под блокировкой на каждый вызов (котировка DEX — десятки вызовов, у MEXC тысячи пулов);
//! теперь — только пулы пары или монеты. Индексы держатся при каждой записи (insert, update, retain).
//! Кто вызывает: pools/registry.rs (под своей блокировкой).
//! От чего зависит: registry.rs (Entry), core/types (PoolRef).
//! Настройки: нет.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use alloy_primitives::Address;

use crate::core::types::PoolRef;

use super::registry::Entry;

/// Записи реестра и индексы по ним.
#[derive(Default)]
pub struct Pools {
    /// Записи пулов. За `Arc`: снимок всех записей (граф маршрутов) не копирует их, запись меняется на месте
    /// (`Arc::make_mut` копирует запись, только если снимок с ней ещё жив).
    map: HashMap<PoolRef, Arc<Entry>>,
    /// (монета, встречный токен) → пулы.
    pairs: HashMap<(Address, Address), HashSet<PoolRef>>,
    /// Монета (адрес контракта) → пулы (решение 211).
    coins: HashMap<Address, HashSet<PoolRef>>,
}

/// Ключи индексов записи: пара и монета.
fn keys(e: &Entry) -> ((Address, Address), Address) {
    ((e.coin, e.usdt), e.coin)
}

impl Pools {
    /// Запись по ссылке. Ошибок нет.
    pub fn get(&self, r: &PoolRef) -> Option<&Arc<Entry>> {
        self.map.get(r)
    }

    /// Есть ли пул. Ошибок нет.
    pub fn contains(&self, r: &PoolRef) -> bool {
        self.map.contains_key(r)
    }

    /// Все записи. Ошибок нет.
    pub fn values(&self) -> impl Iterator<Item = &Arc<Entry>> {
        self.map.values()
    }

    /// Все ссылки. Ошибок нет.
    pub fn refs(&self) -> impl Iterator<Item = &PoolRef> {
        self.map.keys()
    }

    /// Пулы пары «монета — встречный токен» (по индексу). Ошибок нет.
    pub fn pair(&self, coin: Address, quote: Address) -> impl Iterator<Item = &Arc<Entry>> {
        self.pairs
            .get(&(coin, quote))
            .into_iter()
            .flatten()
            .filter_map(|r| self.map.get(r))
    }

    /// Пулы монеты по адресу контракта (по индексу, решение 211). Ошибок нет.
    pub fn coin(&self, coin: &Address) -> impl Iterator<Item = &Arc<Entry>> {
        self.coins
            .get(coin)
            .into_iter()
            .flatten()
            .filter_map(|r| self.map.get(r))
    }

    /// Решение 217: содержимое замороженного реестра из снимка записей (те же `Arc`, без копирования записей).
    /// Принимает: записи снимка. Возвращает: записи с индексами. Ошибок нет.
    pub fn from_arcs(v: Vec<Arc<Entry>>) -> Pools {
        let mut p = Pools::default();
        for e in v {
            let r = e.pool.reference.clone();
            p.index(&r, &e);
            p.map.insert(r, e);
        }
        p
    }

    /// Добавляет или заменяет запись. Ошибок нет.
    pub fn insert(&mut self, e: Entry) {
        let r = e.pool.reference.clone();
        if let Some(old) = self.map.get(&r).cloned() {
            self.unindex(&r, &old);
        }
        self.index(&r, &e);
        self.map.insert(r, Arc::new(e));
    }

    /// Изменение записи на месте; индексы правятся, если функция сменила монету или встречный токен.
    /// Возвращает: результат функции; None — пула нет.
    pub fn update<T>(&mut self, r: &PoolRef, f: impl FnOnce(&mut Entry) -> T) -> Option<T> {
        let slot = self.map.get_mut(r)?;
        let before = keys(slot);
        let e = Arc::make_mut(slot);
        let out = f(e);
        let after = keys(e);
        if after != before {
            let e = e.clone();
            self.unkeys(r, &before);
            self.index(r, &e);
        }
        Some(out)
    }

    /// Оставляет записи, для которых `keep` — да; индексы снятых записей правятся. Ошибок нет.
    pub fn retain(&mut self, keep: impl Fn(&Entry) -> bool) {
        let gone: Vec<(PoolRef, Arc<Entry>)> = self
            .map
            .iter()
            .filter(|(_, e)| !keep(e))
            .map(|(r, e)| (r.clone(), e.clone()))
            .collect();
        for (r, e) in gone {
            self.unindex(&r, &e);
            self.map.remove(&r);
        }
    }

    /// Ставит пул в индексы. Ошибок нет.
    fn index(&mut self, r: &PoolRef, e: &Entry) {
        let (pair, coin) = keys(e);
        self.pairs.entry(pair).or_default().insert(r.clone());
        self.coins.entry(coin).or_default().insert(r.clone());
    }

    /// Снимает пул из индексов по записи. Ошибок нет.
    fn unindex(&mut self, r: &PoolRef, e: &Entry) {
        let k = keys(e);
        self.unkeys(r, &k);
    }

    /// Снимает пул из индексов по ключам; пустые наборы удаляются. Ошибок нет.
    fn unkeys(&mut self, r: &PoolRef, (pair, coin): &((Address, Address), Address)) {
        if let Some(s) = self.pairs.get_mut(pair) {
            s.remove(r);
            if s.is_empty() {
                self.pairs.remove(pair);
            }
        }
        if let Some(s) = self.coins.get_mut(coin) {
            s.remove(r);
            if s.is_empty() {
                self.coins.remove(coin);
            }
        }
    }
}
