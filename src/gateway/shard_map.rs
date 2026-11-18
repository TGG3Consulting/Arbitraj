//! Файл: gateway/shard_map.rs
//! Что делает: решение 409 (ревью t194, Н1–Н2, М3) — карта, разделённая на части (шарды) по хешу ключа: у каждой части
//! свой RwLock. Нужна там, где карту читают и пополняют на каждый пул из многих потоков: один общий RwLock — все
//! потоки бьются за один счётчик читателей (perf 18.11), а снимок за ArcSwap — полная копия карты на каждую новую
//! запись (квадратичная копия при наполнении). Здесь вставка — O(1) в своей части, чтение — замок своей части.
//! Число частей — число потоков процессора машины (std::thread::available_parallelism; факт машины, не настройка);
//! узнаётся один раз за процесс (OnceLock); не удалось узнать — одна часть (поведение как у обычной карты под замком).
//! Кто вызывает: venues/bsc_dex/check/pool_facts_store.rs, venues/bsc_dex/quote/usable_reuse.rs,
//! venues/bsc_dex/pools/registry_key.rs.
//! Решение 410 (t195): хеш — foldhash (быстрый, со случайным зерном процесса; см. venues/bsc_dex/fast_hash.rs —
//! почему ключам не нужен SipHash) для выбора части и внутри части; порядок обхода (for_each) и раньше был случайным.
//! От чего зависит: std, foldhash.
//! Настройки: нет.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hash};

use foldhash::fast::RandomState;

/// Часть карты: обычная карта с хешем foldhash (решение 410).
type Part<K, V> = HashMap<K, V, RandomState>;
use std::sync::{OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Карта по частям.
pub struct ShardMap<K, V> {
    shards: Box<[RwLock<Part<K, V>>]>,
    hash: RandomState,
}

/// Что делает: число частей — потоки процессора, узнаётся один раз за процесс (ревью t194, М2: available_parallelism
/// — системный вызов и чтение cgroup, а карта создаётся на каждый подбор объёма). Не удалось узнать — одна часть.
/// Ошибок нет.
fn parts() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
}

impl<K: Hash + Eq, V> Default for ShardMap<K, V> {
    /// Что делает: пустая карта, частей — по числу потоков процессора. Ошибок нет.
    fn default() -> Self {
        let n = parts();
        Self {
            shards: (0..n).map(|_| RwLock::new(Part::default())).collect(),
            hash: RandomState::default(),
        }
    }
}

impl<K: Hash + Eq, V> ShardMap<K, V> {
    /// Что делает: часть карты для ключа. Ошибок нет.
    fn shard<Q: Hash + ?Sized>(&self, k: &Q) -> &RwLock<Part<K, V>> {
        let i = self.hash.hash_one(k) as usize % self.shards.len();
        &self.shards[i]
    }

    /// Что делает: чтение части (отравленный замок — данные как есть). Ошибок нет.
    fn read_of<Q: Hash + ?Sized>(&self, k: &Q) -> RwLockReadGuard<'_, Part<K, V>> {
        self.shard(k).read().unwrap_or_else(|p| p.into_inner())
    }

    /// Что делает: запись части (отравленный замок — данные как есть). Ошибок нет.
    fn write_of<Q: Hash + ?Sized>(&self, k: &Q) -> RwLockWriteGuard<'_, Part<K, V>> {
        self.shard(k).write().unwrap_or_else(|p| p.into_inner())
    }

    /// Что делает: значение по ключу, переданное в f (под замком чтения своей части). Принимает: ключ, f.
    /// Возвращает: итог f. Ошибок нет.
    pub fn with<Q, R>(&self, k: &Q, f: impl FnOnce(Option<&V>) -> R) -> R
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        f(self.read_of(k).get(k))
    }

    /// Что делает: копия значения по ключу. Возвращает: значение или None. Ошибок нет.
    pub fn get<Q>(&self, k: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
        V: Clone,
    {
        self.with(k, |v| v.cloned())
    }

    /// Что делает: кладёт значение (прежнее заменяется). Принимает: ключ, значение. Возвращает: прежнее. Ошибок нет.
    pub fn insert(&self, k: K, v: V) -> Option<V> {
        self.write_of(&k).insert(k, v)
    }

    /// Что делает: значение по ключу; нет — кладёт новое из make (под замком записи своей части, повторная проверка).
    /// Принимает: ключ, конструктор. Возвращает: копию значения. Ошибок нет.
    pub fn get_or_insert_with(&self, k: K, make: impl FnOnce() -> V) -> V
    where
        V: Clone,
    {
        if let Some(v) = self.get(&k) {
            return v;
        }
        let mut g = self.write_of(&k);
        g.entry(k).or_insert_with(make).clone()
    }

    /// Что делает: обход всех записей (часть за частью, под замком чтения части). Принимает: f. Ошибок нет.
    pub fn for_each(&self, mut f: impl FnMut(&K, &V)) {
        for s in self.shards.iter() {
            for (k, v) in s.read().unwrap_or_else(|p| p.into_inner()).iter() {
                f(k, v);
            }
        }
    }
}
