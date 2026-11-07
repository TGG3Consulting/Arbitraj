//! Файл: venues/bsc_dex/pools/registry_frozen.rs
//! Что делает: решение 217 — снимок реестра пулов на момент решения о входе в сделку и замороженный реестр из него.
//! Снимок — только `Arc` записей (записи неизменяемы: запись реестра меняется через `Arc::make_mut`, которая копирует
//! запись, пока её держит снимок; карты тиков и бинов — так же). Дорогая часть (индексы) строится уже в фоновом
//! расчёте (frozen_from), путь сделки платит только за копию `Arc` нужных записей (десятки, одна короткая блокировка на
//! чтение) и вердиктов глубины этих пулов. Снимаются (исправления по проверке 217 — без полного реестра в любом режиме):
//! - пулы монеты (с USDT и с промежуточными монетами) и пулы «промежуточная — USDT» — как прямой режим;
//! - пулы «WBNB — USDT» (dex.native_wrapped): цена газа в USDT при выборе маршрута (quote/pick.rs, route_gas.rs) — как
//!   у живой котировки, даже если WBNB не среди промежуточных;
//! - при dex.router_mode = graph — пулы путей, которые живой кэш графа уже нашёл для монеты (quote/route_cache.rs
//!   lookup); сами пути закрепляются в замороженном реестре (graph_cache.rs pinned) — граф заново не строится.
//!
//! Глубина пулов (решение 73) у замороженного реестра — своя книга, которая не пишет журнал и никого не будит, а
//! прошлый вердикт гистерезиса — копия из живой книги на момент снимка (depth_book.rs verdicts, seeded).
//! Кто вызывает: venues/bsc_dex/frozen.rs.
//! От чего зависит: registry.rs, registry_index.rs, depth_book.rs, graph_cache.rs (Cache, Skeleton).
//! Настройки: нет.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use alloy_primitives::Address;

use crate::core::types::PoolRef;

use super::depth_book::{Book, Verdicts};
use super::graph::{Cache, PathsKey, Skeleton};
use super::registry::{Entry, Registry};
use super::registry_index::Pools;

/// Что снять для котировки монеты (см. шапку).
pub struct Want<'a> {
    pub coin: Address,
    /// Промежуточные монеты (решение 52).
    pub mids: &'a [Address],
    /// Встречный токен (USDT).
    pub usdt: Address,
    /// Обёртка нативной монеты (WBNB) — пулы «WBNB — USDT» для цены газа; None — не задана.
    pub wbnb: Option<Address>,
    /// Скелеты путей графа монеты на этот момент (пусто — прямой режим или в кэше графа путей нет).
    pub paths: &'a [Skeleton],
}

/// Снимок: `Arc` записей (без повторов) и вердикты глубины этих пулов на момент снимка.
pub struct Snap {
    pub entries: Vec<Arc<Entry>>,
    pub verdicts: Verdicts,
}

impl Registry {
    /// Что делает: снимок записей для котировки монеты на этот момент (см. шапку).
    /// Принимает: что снять. Возвращает: снимок (без копирования самих записей). Ошибок нет.
    pub fn freeze(&self, w: &Want) -> Snap {
        let mut seen: HashSet<PoolRef> = HashSet::new();
        let mut entries: Vec<Arc<Entry>> = Vec::new();
        {
            let m = self.inner.read().unwrap_or_else(|p| p.into_inner());
            let mut add = |e: &Arc<Entry>| {
                if seen.insert(e.pool.reference.clone()) {
                    entries.push(e.clone());
                }
            };
            m.coin(&w.coin).for_each(&mut add);
            for a in w.mids.iter().chain(w.wbnb.iter()) {
                m.pair(*a, w.usdt).for_each(&mut add);
            }
            for (r, _, _) in w.paths.iter().flatten() {
                if let Some(e) = m.get(r) {
                    add(e);
                }
            }
        }
        // Блокировка реестра уже снята: вердикты — под своей короткой блокировкой книги глубины.
        let verdicts = self.depth.verdicts(seen.iter());
        Snap { entries, verdicts }
    }

    /// Что делает: замороженный реестр из снимка: свои индексы, книга глубины с вердиктами момента снимка (без журнала
    /// и сигналов), кэш графа — с закреплённым набором путей монеты (если он был). Версия — 0 и не меняется.
    /// Принимает: снимок, закреплённые пути (ключ кэша и скелеты) или None. Возвращает: реестр. Ошибок нет.
    pub fn frozen_from(snap: &Snap, pinned: Option<(PathsKey, Arc<Vec<Skeleton>>)>) -> Registry {
        Registry {
            inner: Arc::new(std::sync::RwLock::new(Pools::from_arcs(
                snap.entries.clone(),
            ))),
            depth: Book::seeded(snap.verdicts.clone()),
            version: Arc::new(AtomicU64::new(0)),
            graph: pinned.map_or_else(Cache::default, |(k, v)| Cache::pinned(k, v)),
        }
    }
}
