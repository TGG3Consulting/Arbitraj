//! Файл: venues/bsc_dex/check/pool_facts_store.rs
//! Что делает: решение 409 (t194, п. а) — факты пулов из проверок (PoolFacts) в памяти без замка на чтение. Раньше —
//! `RwLock<HashMap<PoolRef, PoolFacts>>` и полная копия фактов на каждое чтение: котировка (quote/select.rs
//! usable_with) читала их на каждый пул, и потоки бились за счётчик читателей замка (perf 18.11). Теперь: карта «пул →
//! ячейка» — снимок за ArcSwap (меняется только при появлении нового пула), ячейка пула — ArcSwap<PoolFacts> (факты за
//! Arc: читатель берёт указатель, без копии). Запись (update) — по одной под замком писателя: текущие факты → копия →
//! правка → новые факты в ячейку; нового пула нет в карте — новая карта с его ячейкой. Как раньше (entry().or_default()):
//! запись пула появляется даже если правка ничего не записала, и правка сохраняется целиком, что бы она ни вернула.
//! Кто вызывает: check/state.rs (Checks).
//! От чего зависит: check/state.rs (PoolFacts), core/types (PoolRef), arc-swap.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;

use crate::core::types::PoolRef;

use super::state::PoolFacts;

/// Ячейка фактов одного пула.
type Cell = Arc<ArcSwap<PoolFacts>>;

/// Факты пулов: снимок карты ячеек и замок писателей.
pub struct PoolFactsStore {
    map: ArcSwap<HashMap<PoolRef, Cell>>,
    write: Mutex<()>,
}

impl Default for PoolFactsStore {
    /// Что делает: пустое хранилище. Ошибок нет.
    fn default() -> Self {
        Self::from_map(HashMap::new())
    }
}

impl PoolFactsStore {
    /// Что делает: хранилище из готовых фактов (копия на момент снимка, решение 239 — свои ячейки, общие только Arc
    /// фактов: новые записи живого хранилища в копию не попадают). Принимает: пул → факты. Ошибок нет.
    pub fn from_map(m: HashMap<PoolRef, Arc<PoolFacts>>) -> Self {
        let cells = m
            .into_iter()
            .map(|(r, f)| (r, Arc::new(ArcSwap::new(f))))
            .collect();
        Self {
            map: ArcSwap::from_pointee(cells),
            write: Mutex::new(()),
        }
    }

    /// Что делает: факты пула без замка и без копии. Принимает: пул. Возвращает: факты или None — проверок не было.
    /// Ошибок нет.
    pub fn get(&self, r: &PoolRef) -> Option<Arc<PoolFacts>> {
        self.map.load().get(r).map(|c| c.load_full())
    }

    /// Что делает: обход всех пулов (пул, факты). Принимает: функцию. Ошибок нет.
    pub fn for_each(&self, mut f: impl FnMut(&PoolRef, &PoolFacts)) {
        for (r, c) in self.map.load().iter() {
            f(r, &c.load());
        }
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
        let map = self.map.load_full();
        let cell = map.get(r).cloned();
        let mut facts = cell
            .as_ref()
            .map(|c| PoolFacts::clone(&c.load()))
            .unwrap_or_default();
        let wrote = f(&mut facts);
        let facts = Arc::new(facts);
        match cell {
            Some(c) => c.store(facts),
            None => {
                let mut next = (*map).clone();
                next.insert(r.clone(), Arc::new(ArcSwap::new(facts)));
                self.map.store(Arc::new(next));
            }
        }
        if wrote {
            after();
        }
        wrote
    }
}
