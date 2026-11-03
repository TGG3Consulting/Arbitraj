//! Файл: venues/bsc_dex/pools/registry.rs
//! Что делает: реестр пулов адаптера в памяти — пул в формате ядра плюс то, что нужно только адаптеру:
//! монета и встречный токен пула (USDT или промежуточная монета), ключ V4, карта тиков, резервы бинов, блок синхронизации, достоверность.
//! Кто вызывает: pools/market.rs, pools/listen.rs, pools/events_*.rs (пишут), quote/*, check/*, exec/* (читают).
//! От чего зависит: core/types (Pool, PoolRef), pools/families.rs, key.rs, ticks.rs, bins.rs.
//! Решение 129: номер версии реестра (растёт при любой записи — put, update, retain) — по нему граф маршрутов строится
//! один раз на состояние реестра, а не на каждую котировку (pools/graph.rs cached); pair_marks — отметки синхронизации
//! пулов пары без копирования записей (ключ кэша глубины, pools/depth_key.rs).
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// Тикер монеты пула.
    pub ticker: String,
    /// Адрес монеты пула.
    pub coin: Address,
    /// Встречный токен пула: USDT или промежуточная монета (решение 52) — пулы «монета — промежуточная»
    /// и «промежуточная — USDT» (у второй монета — промежуточная, встречный — USDT).
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
    /// Пул мельче dex.min_pool_liquidity_usd: слушается только добавление ликвидности (pools/depth.rs), обмены не
    /// приходят — состояние может отставать. Такой пул не котируется (quote/select.rs) до возврата в полную подписку.
    pub asleep: bool,
}

/// Реестр пулов. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Registry {
    inner: Arc<RwLock<HashMap<PoolRef, Entry>>>,
    /// Глубина пулов по направлениям (решение 73, depth_book.rs).
    depth: super::depth_book::Book,
    /// Версия: растёт при любой записи (решение 129).
    version: Arc<AtomicU64>,
    /// Граф маршрутов последней версии (решение 129, graph.rs cached).
    graph: super::graph::Cache,
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
    /// Глубина пулов по направлениям (решение 73). Ошибок нет.
    pub fn depth(&self) -> &super::depth_book::Book {
        &self.depth
    }

    /// Версия реестра (решение 129): растёт при любой записи. Ошибок нет.
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// Кэш графа маршрутов (решение 129, graph.rs). Ошибок нет.
    pub fn graph_cache(&self) -> &super::graph::Cache {
        &self.graph
    }

    /// Добавляет или заменяет запись. Ошибок нет.
    pub fn put(&self, e: Entry) {
        let mut m = self.inner.write().unwrap_or_else(|p| p.into_inner());
        m.insert(e.pool.reference.clone(), e);
        self.version.fetch_add(1, Ordering::AcqRel);
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
        // Версия — до снятия блокировки: читатель, увидевший новое состояние, увидит и новую версию.
        self.version.fetch_add(1, Ordering::AcqRel);
        m.get_mut(r).map(f)
    }

    /// Пулы пары «монета — встречный токен» (копии). Ошибок нет.
    pub fn by_pair(&self, coin: Address, quote: Address) -> Vec<Entry> {
        let m = self.inner.read().unwrap_or_else(|p| p.into_inner());
        m.values()
            .filter(|e| e.coin == coin && e.usdt == quote)
            .cloned()
            .collect()
    }

    /// Отметки синхронизации пулов пары «монета — встречный токен»: (блок, logIndex, достоверен) — без копирования
    /// записей (решение 129: ключ кэша глубины считается для каждого пула при построении графа). Ошибок нет.
    pub fn pair_marks(&self, coin: Address, quote: Address) -> Vec<(u64, Option<u64>, bool)> {
        let m = self.inner.read().unwrap_or_else(|p| p.into_inner());
        m.values()
            .filter(|e| e.coin == coin && e.usdt == quote)
            .map(|e| (e.synced_block, e.synced_log, e.valid))
            .collect()
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

    /// Копии всех записей пулов (граф маршрутов, решение 80). Ошибок нет.
    pub fn all(&self) -> Vec<Entry> {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect()
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
        self.version.fetch_add(1, Ordering::AcqRel);
    }
}
