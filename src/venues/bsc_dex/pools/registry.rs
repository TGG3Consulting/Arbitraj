//! Файл: venues/bsc_dex/pools/registry.rs
//! Что делает: реестр пулов адаптера в памяти — пул в формате ядра плюс то, что нужно только адаптеру:
//! монета пула, ключ V4, карта тиков, резервы бинов, блок синхронизации, достоверность.
//! Кто вызывает: pools/market.rs, pools/listen.rs, pools/events_*.rs (пишут), quote/*, check/*, exec/* (читают).
//! От чего зависит: core/types (Pool, PoolRef), pools/families.rs, key.rs, ticks.rs, bins.rs.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use alloy_primitives::Address;

use crate::core::types::{Pool, PoolRef};

use super::bins::BinMap;
use super::families::Family;
use super::key::PoolKey;
use super::ticks::TickMap;

/// Запись о пуле.
#[derive(Debug, Clone)]
pub struct Entry {
    pub family: &'static Family,
    /// Пул в формате ядра (состояние, блок, крючок).
    pub pool: Pool,
    /// Тикер монеты пула (вторая валюта — USDT).
    pub ticker: String,
    /// Адрес монеты пула.
    pub coin: Address,
    /// Адрес USDT.
    pub usdt: Address,
    /// Ключ V4 (CL, Bin); у V2/V3 — None.
    pub key: Option<PoolKey>,
    /// V3/CL: карта тиков; иначе None.
    pub ticks: Option<TickMap>,
    /// Bin: резервы бинов; иначе None.
    pub bins: Option<BinMap>,
    /// Последний учтённый блок: состояние верно на конец этого блока (synced_log = None) или сразу после
    /// события с этим logIndex в нём (synced_log = Some).
    pub synced_block: u64,
    /// logIndex последнего применённого события в synced_block; None — учтён весь блок (состояние прочитано на нём).
    pub synced_log: Option<u64>,
    /// false — подписка оборвалась или карта разошлась с событием: данным верить нельзя до перечитывания.
    pub valid: bool,
}

/// Реестр пулов. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Registry {
    inner: Arc<RwLock<HashMap<PoolRef, Entry>>>,
}

impl Entry {
    /// Учтено ли уже событие с позицией (блок, logIndex). Порядок событий — (blockNumber, logIndex)
    /// (execution-apis src/schemas/receipt.yaml, Log.logIndex — «index of the log in the block»).
    /// Ошибок нет.
    pub fn has_applied(&self, block: u64, log_index: u64) -> bool {
        block < self.synced_block
            || (block == self.synced_block && self.synced_log.is_none_or(|i| log_index <= i))
    }
}

impl Registry {
    /// Добавляет или заменяет запись. Ошибок нет.
    pub fn put(&self, e: Entry) {
        let mut m = self.inner.write().unwrap_or_else(|p| p.into_inner());
        m.insert(e.pool.reference.clone(), e);
    }

    /// Копия записи. None — пула нет.
    pub fn get(&self, r: &PoolRef) -> Option<Entry> {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(r)
            .cloned()
    }

    /// Изменение записи на месте. Принимает: ссылку и функцию. Возвращает: её результат; None — пула нет.
    pub fn update<T>(&self, r: &PoolRef, f: impl FnOnce(&mut Entry) -> T) -> Option<T> {
        let mut m = self.inner.write().unwrap_or_else(|p| p.into_inner());
        m.get_mut(r).map(f)
    }

    /// Все пулы монеты (копии). Ошибок нет.
    pub fn by_ticker(&self, ticker: &str) -> Vec<Entry> {
        let m = self.inner.read().unwrap_or_else(|p| p.into_inner());
        m.values().filter(|e| e.ticker == ticker).cloned().collect()
    }

    /// Есть ли такой пул. Ошибок нет.
    pub fn contains(&self, r: &PoolRef) -> bool {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(r)
    }

    /// Ссылки на все пулы. Ошибок нет.
    pub fn refs(&self) -> Vec<PoolRef> {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .keys()
            .cloned()
            .collect()
    }

    /// Удаляет пулы монет, которых больше нет в подписке. Принимает: функцию «оставить ли». Ошибок нет.
    pub fn retain(&self, keep: impl Fn(&Entry) -> bool) {
        self.inner
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|_, e| keep(e));
    }
}
