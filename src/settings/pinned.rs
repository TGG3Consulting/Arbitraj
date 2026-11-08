//! Файл: settings/pinned.rs
//! Что делает: решение 239 — копия всех значений настроек на момент входа в сделку (core/frozen.rs take, снимок DEX
//! venues/bsc_dex/frozen.rs take): расчёт «без потолка» по сделке читает пороги и параметры модели (money.*, size.*,
//! uncapped.*, card.max_search_usdt, profit.*, venue.quote_asset, настройки котировки DEX) такими, какими они были в
//! момент решения, а не на момент фонового расчёта. Значения в памяти хранилища — `Arc` с копией при записи
//! (store.rs set: Arc::make_mut): снимок — один клон указателя (без копирования значений на пути сделки), полная копия
//! делается только при изменении настройки, пока жив хоть один снимок. Копия только для чтения: set отказывает; история,
//! журнал и рассылка изменений к ней не относятся. Кошельки и маппинг (accounts) — общие с живым хранилищем.
//! Кто вызывает: core/frozen.rs, venues/bsc_dex/frozen.rs.
//! От чего зависит: settings/store.rs.
//! Настройки: нет (копирует все).

use std::sync::{Arc, RwLock};

use super::store::Store;

impl Store {
    /// Что делает: копия значений настроек на этот момент (см. шапку). Возвращает: Store только для чтения.
    /// Ошибок нет.
    pub fn pinned(&self) -> Store {
        let now = self.cache.read().unwrap_or_else(|p| p.into_inner()).clone();
        let mut copy = self.clone();
        copy.cache = Arc::new(RwLock::new(now));
        copy.pinned = true;
        copy
    }
}
