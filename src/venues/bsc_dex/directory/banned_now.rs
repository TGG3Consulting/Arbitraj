//! Файл: venues/bsc_dex/directory/banned_now.rs
//! Что делает: решение 409 (t194, п. г) — снимок исключённых пулов (проверка «кто меняет», Snap.banned) для горячего
//! пути котировки: quote/select.rs спрашивает Directory::banned на каждый пул, и раньше это было чтение под RwLock
//! всего справочника с копией строки причины. Здесь — карта «физический пул → причина» за ArcSwap (чтение без замка),
//! причина Arc<str> (без копии строки). Пишет только Directory::ban под замком справочника, вслед за Snap.banned,
//! поэтому порядок записей и содержимое те же.
//! Кто вызывает: directory/mod.rs (ban, banned).
//! От чего зависит: core/types (PoolRef), arc-swap.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::core::types::PoolRef;

/// Исключённые пулы: физический пул (`PoolRef::site`) → причина.
pub struct BannedNow(ArcSwap<HashMap<PoolRef, Arc<str>>>);

impl Default for BannedNow {
    /// Что делает: пустой снимок. Ошибок нет.
    fn default() -> Self {
        Self(ArcSwap::from_pointee(HashMap::new()))
    }
}

impl BannedNow {
    /// Что делает: причина исключения физического пула. Принимает: `PoolRef::site()` пула. Возвращает: причину или
    /// None. Ошибок нет.
    pub fn get(&self, site: &PoolRef) -> Option<Arc<str>> {
        self.0.load().get(site).cloned()
    }

    /// Что делает: новый снимок с записью (вызывать под замком справочника — писатель один). Принимает: физический
    /// пул, причину. Ошибок нет.
    pub fn put(&self, site: PoolRef, why: &str) {
        let mut next = (*self.0.load_full()).clone();
        next.insert(site, Arc::from(why));
        self.0.store(Arc::new(next));
    }
}
