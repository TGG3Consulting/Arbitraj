//! Файл: venues/bsc_dex/pools/registry_frozen.rs
//! Что делает: решение 217 — снимок реестра пулов на момент решения о входе в сделку и замороженный реестр из него.
//! Снимок — только `Arc` записей (записи неизменяемы: запись реестра меняется через `Arc::make_mut`, которая копирует
//! запись, пока её держит снимок; карты тиков и бинов — так же). Дорогая часть (индексы, граф маршрутов) строится уже
//! в фоновом расчёте (frozen_from), путь сделки платит только за копию `Arc` нужных записей:
//! - прямой режим (dex.router_mode ≠ graph): пулы монеты (с USDT и с промежуточными монетами) и пулы
//!   «промежуточная — USDT» — десятки записей;
//! - граф (dex.router_mode = graph): все записи реестра (маршрут может идти через любые пулы).
//!
//! Глубина пулов (решение 73) у замороженного реестра — своя книга, которая не пишет журнал и никого не будит, а
//! прошлый вердикт гистерезиса берёт из живой книги (depth_book.rs seeded).
//! Кто вызывает: venues/bsc_dex/frozen.rs.
//! От чего зависит: registry.rs, registry_index.rs, depth_book.rs, graph.rs (Cache).
//! Настройки: нет.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use alloy_primitives::Address;

use super::registry::{Entry, Registry};
use super::registry_index::Pools;

impl Registry {
    /// Что делает: снимок записей для котировки монеты на этот момент (см. шапку).
    /// Принимает: адрес монеты, адреса промежуточных монет, их встречный токен (USDT), все ли записи (граф).
    /// Возвращает: `Arc` записей (без копирования самих записей). Ошибок нет.
    pub fn freeze(
        &self,
        coin: Address,
        mids: &[Address],
        usdt: Address,
        all: bool,
    ) -> Vec<Arc<Entry>> {
        if all {
            return self.snapshot();
        }
        let m = self.inner.read().unwrap_or_else(|p| p.into_inner());
        let mut v: Vec<Arc<Entry>> = m.coin(&coin).cloned().collect();
        for a in mids {
            v.extend(m.pair(*a, usdt).cloned());
        }
        v
    }

    /// Что делает: замороженный реестр из снимка записей: свои индексы, свой кэш графа, своя книга глубины с прошлым
    /// вердиктом из живой (без журнала и сигналов). Версия — 0 и не меняется (записей в него нет).
    /// Принимает: записи снимка. Возвращает: реестр. Ошибок нет.
    pub fn frozen_from(&self, entries: Vec<Arc<Entry>>) -> Registry {
        Registry {
            inner: Arc::new(std::sync::RwLock::new(Pools::from_arcs(entries))),
            depth: self.depth.seeded(),
            version: Arc::new(AtomicU64::new(0)),
            graph: Default::default(),
        }
    }
}
