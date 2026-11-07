//! Файл: venues/bsc_dex/pools/registry.rs
//! Что делает: реестр пулов адаптера в памяти — пул в формате ядра плюс то, что нужно только адаптеру:
//! монета и встречный токен пула (USDT или промежуточная монета), ключ V4, карта тиков, резервы бинов, блок синхронизации, достоверность.
//! Кто вызывает: pools/market.rs, pools/listen.rs, pools/events_*.rs (пишут), quote/*, check/*, exec/* (читают).
//! От чего зависит: core/types (Pool, PoolRef), pools/families.rs, key.rs, ticks.rs, bins.rs, registry_index.rs.
//! Решение 129: номер версии реестра (растёт при любой записи — put, update, retain) — по нему граф маршрутов строится
//! один раз на состояние реестра, а не на каждую котировку (pools/graph.rs cached); pair_marks — отметки синхронизации
//! пулов пары без копирования записей (ключ кэша глубины, pools/depth_key.rs). Записи — за `Arc`, пары и тикеры —
//! по индексам (registry_index.rs): by_pair / by_coin / pair_marks не проходят весь реестр; карты тиков и бинов
//! общие при копии записи (ticks.rs, bins.rs); snapshot — все записи без копирования (граф маршрутов). Блокировка
//! держится только на чтение индекса и копию нужных записей — счёт котировки идёт без неё.
//! Настройки: нет.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use alloy_primitives::Address;

use crate::core::types::{Pool, PoolRef};

use super::bins::BinMap;
use super::families::Family;
use super::key::PoolKey;
use super::registry_index::Pools;
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
    /// Записи за `Arc` и индексы по паре и тикеру (registry_index.rs, решение 129).
    inner: Arc<RwLock<Pools>>,
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

    /// Чтение под блокировкой (короткое: только индекс и копии нужных записей). Ошибок нет.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, Pools> {
        self.inner.read().unwrap_or_else(|p| p.into_inner())
    }

    /// Добавляет или заменяет запись. Ошибок нет.
    pub fn put(&self, e: Entry) {
        let mut m = self.inner.write().unwrap_or_else(|p| p.into_inner());
        m.insert(e);
        self.version.fetch_add(1, Ordering::AcqRel);
    }

    /// Копия записи. None — пула нет.
    pub fn get(&self, r: &PoolRef) -> Option<Entry> {
        self.read().get(r).map(|e| (**e).clone())
    }

    /// Изменение записи на месте. Принимает: ссылку и функцию. Возвращает: её результат; None — пула нет.
    pub fn update<T>(&self, r: &PoolRef, f: impl FnOnce(&mut Entry) -> T) -> Option<T> {
        let mut m = self.inner.write().unwrap_or_else(|p| p.into_inner());
        // Версия — до снятия блокировки: читатель, увидевший новое состояние, увидит и новую версию.
        self.version.fetch_add(1, Ordering::AcqRel);
        m.update(r, f)
    }

    /// Пулы пары «монета — встречный токен» (копии; карты тиков и бинов — общие, за `Arc`). По индексу пары, а не
    /// проходом реестра (решение 129). Ошибок нет.
    pub fn by_pair(&self, coin: Address, quote: Address) -> Vec<Entry> {
        self.read()
            .pair(coin, quote)
            .map(|e| (**e).clone())
            .collect()
    }

    /// Отметки синхронизации пулов пары «монета — встречный токен»: (блок, logIndex, достоверен) — без копирования
    /// записей (решение 129: ключ кэша глубины считается для каждого пула при построении графа). Ошибок нет.
    pub fn pair_marks(&self, coin: Address, quote: Address) -> Vec<(u64, Option<u64>, bool)> {
        self.read()
            .pair(coin, quote)
            .map(|e| (e.synced_block, e.synced_log, e.valid))
            .collect()
    }

    /// Все пулы монеты (копии) — по адресу контракта (решение 211; раньше по тикеру). Ошибок нет.
    pub fn by_coin(&self, coin: &Address) -> Vec<Entry> {
        self.read().coin(coin).map(|e| (**e).clone()).collect()
    }

    /// Есть ли такой пул. Ошибок нет.
    pub fn contains(&self, r: &PoolRef) -> bool {
        self.read().contains(r)
    }

    /// Копии всех записей пулов. Ошибок нет.
    pub fn all(&self) -> Vec<Entry> {
        self.read().values().map(|e| (**e).clone()).collect()
    }

    /// Снимок всех записей без копирования (решение 129: граф маршрутов строится по нему вне блокировки). Ошибок нет.
    pub fn snapshot(&self) -> Vec<Arc<Entry>> {
        self.read().values().cloned().collect()
    }

    /// Ссылки на все пулы. Ошибок нет.
    pub fn refs(&self) -> Vec<PoolRef> {
        self.read().refs().cloned().collect()
    }

    /// Удаляет пулы монет, которых больше нет в подписке. Принимает: функцию «оставить ли». Ошибок нет.
    pub fn retain(&self, keep: impl Fn(&Entry) -> bool) {
        self.inner
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .retain(keep);
        self.version.fetch_add(1, Ordering::AcqRel);
    }
}
