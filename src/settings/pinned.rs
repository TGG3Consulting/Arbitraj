//! Файл: settings/pinned.rs
//! Что делает: решение 239 — копия всех значений настроек на момент входа в сделку (core/frozen.rs take, снимок DEX
//! venues/bsc_dex/frozen.rs take): расчёт «без потолка» по сделке читает пороги и параметры модели (money.*, size.*,
//! uncapped.*, card.max_search_usdt, profit.*, venue.quote_asset, настройки котировки DEX) такими, какими они были в
//! момент решения, а не на момент фонового расчёта. Значения в памяти хранилища — неизменяемый снимок за ArcSwap
//! (решение 409, settings/snapshot.rs): копия — один клон указателя (без копирования значений на пути сделки); запись
//! настройки всегда публикует новый снимок, копия продолжает держать прежний. Копия только для чтения: set отказывает; история,
//! журнал и рассылка изменений к ней не относятся. Кошельки и маппинг (accounts) — общие с живым хранилищем.
//! Кто вызывает: core/frozen.rs, venues/bsc_dex/frozen.rs.
//! От чего зависит: settings/store.rs.
//! Настройки: нет (копирует все).

use std::sync::Arc;

use super::snapshot::Snapshot;
use super::store::Store;

impl Store {
    /// Что делает: копия значений настроек на этот момент (см. шапку). Возвращает: Store только для чтения.
    /// Ошибок нет.
    pub fn pinned(&self) -> Store {
        // Решение 409: текущий снимок без замка; копия — свой Snapshot с тем же Arc значений.
        let now = self.cache.load_full();
        let mut copy = self.clone();
        copy.cache = Arc::new(Snapshot::new(now));
        copy.pinned = true;
        copy
    }
}
