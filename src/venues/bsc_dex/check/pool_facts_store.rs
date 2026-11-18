//! Файл: venues/bsc_dex/check/pool_facts_store.rs
//! Что делает: решение 409 (t194, п. а; ревью Н2) — факты пулов из проверок (PoolFacts) в памяти без общего замка и без
//! копии на чтение. Раньше — один `RwLock<HashMap<PoolRef, PoolFacts>>` и полная копия фактов на каждое чтение:
//! котировка (quote/select.rs usable_with) читала их на каждый пул, и потоки бились за один счётчик читателей замка
//! (perf 18.11). Теперь: карта «пул → ячейка» разделена на части по хешу пула (gateway/shard_map.rs: замок своей части,
//! новый пул — вставка O(1) в свою часть, без копии карты), ячейка пула — ArcSwap<PoolFacts> (факты за Arc: читатель
//! берёт указатель, без копии). Запись (update) — по одной под замком писателя: текущие факты → копия → правка → новые
//! факты в ячейку. Как раньше (entry().or_default()): запись пула появляется, даже если правка ничего не записала, и
//! правка сохраняется целиком, что бы она ни вернула.
//! Кто вызывает: check/state.rs (Checks).
//! От чего зависит: check/state.rs (PoolFacts), core/types (PoolRef), gateway/shard_map.rs, arc-swap.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;

use crate::core::types::PoolRef;
use crate::gateway::shard_map::ShardMap;

use super::state::PoolFacts;

/// Ячейка фактов одного пула.
type Cell = Arc<ArcSwap<PoolFacts>>;

/// Факты пулов: карта ячеек по частям и замок писателей.
#[derive(Default)]
pub struct PoolFactsStore {
    map: ShardMap<PoolRef, Cell>,
    write: Mutex<()>,
}

impl PoolFactsStore {
    /// Что делает: хранилище из готовых фактов (копия на момент снимка, решение 239 — свои ячейки, общие только Arc
    /// фактов: новые записи живого хранилища в копию не попадают). Принимает: пул → факты. Ошибок нет.
    pub fn from_map(m: HashMap<PoolRef, Arc<PoolFacts>>) -> Self {
        let s = Self::default();
        for (r, f) in m {
            s.map.insert(r, Arc::new(ArcSwap::new(f)));
        }
        s
    }

    /// Что делает: факты пула без копии. Принимает: пул. Возвращает: факты или None — проверок не было. Ошибок нет.
    pub fn get(&self, r: &PoolRef) -> Option<Arc<PoolFacts>> {
        self.map.with(r, |c| c.map(|c| c.load_full()))
    }

    /// Что делает: обход всех пулов (пул, факты). Принимает: функцию. Ошибок нет.
    pub fn for_each(&self, mut f: impl FnMut(&PoolRef, &PoolFacts)) {
        self.map.for_each(|r, c| f(r, &c.load()));
    }

    /// Что делает: правка фактов пула под замком писателя (см. шапку); after — под тем же замком, если правка что-то
    /// записала (вернула true; поколение итогов, решение 352). Принимает: пул, правку, after. Возвращает: итог правки.
    /// Ошибок нет (отравленный замок — продолжаем: под ним нет данных, только очередь писателей).
    pub fn update(
        &self,
        r: &PoolRef,
        f: impl FnOnce(&mut PoolFacts) -> bool,
        after: impl FnOnce(),
    ) -> bool {
        let _turn = self.write.lock().unwrap_or_else(|p| p.into_inner());
        // Ревью t194 (М1): новый пул — ячейка сразу с итогом правки (без окна с пустыми фактами); писатель один (замок
        // выше), поэтому между поиском и вставкой ячейку никто не создаст.
        let cell = self.map.get(r);
        let mut facts = cell
            .as_ref()
            .map(|c| PoolFacts::clone(&c.load()))
            .unwrap_or_default();
        let wrote = f(&mut facts);
        match cell {
            Some(c) => c.store(Arc::new(facts)),
            None => {
                self.map
                    .insert(r.clone(), Arc::new(ArcSwap::from_pointee(facts)));
            }
        }
        if wrote {
            after();
        }
        wrote
    }
}
