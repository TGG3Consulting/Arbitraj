//! Хранение кадров FeeRouter и версий модуля Motoswap. Обе проверки в motoswap_frame.rs
//! получают одну ячейку на совпавший ключ. Окружение площадки владеет кэшем, а точный
//! DEX ID задаёт отдельный лимит. Зависит от настроек этой области и tokio::sync::OnceCell.
use super::{Address, B256, Ctx, Frame, Roles};
use crate::settings::{scope::Scope, store::Store};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    hash::Hash,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::OnceCell;

const LIMIT: &str = "dex.motoswap_cache_entries";
type Cell<T> = Arc<OnceCell<T>>;
type Version = (B256, Address, Vec<B256>);

// Слабая ссылка позволяет присоединиться к чтению, пока его держит хотя бы один
// вызывающий. Ограничение относится только к сильным ссылкам, сохраняющим результат.
struct Entries<K, T> {
    kept: HashMap<Arc<K>, Cell<T>>,
    fifo: VecDeque<Arc<K>>,
    pending: HashMap<Arc<K>, Weak<OnceCell<T>>>,
}
impl<K, T> Default for Entries<K, T> {
    fn default() -> Self {
        Self { kept: HashMap::new(), fifo: VecDeque::new(), pending: HashMap::new() }
    }
}
impl<K: Eq + Hash, T> Entries<K, T> {
    /// Освобождает самые ранние записи сверх лимита, сохраняя доступ к незавершённым чтениям.
    fn trim(&mut self, limit: usize) {
        while self.fifo.len() > limit {
            let key = self.fifo.pop_front().expect("проверена длина очереди");
            let cell = self.kept.remove(&key).expect("индекс и очередь изменяются вместе");
            if Arc::strong_count(&cell) > 1 {
                self.pending.insert(key, Arc::downgrade(&cell));
            }
        }
    }

    /// Возвращает память после уменьшения оператором лимита. Активные чтения остаются доступны.
    fn reduce(&mut self, limit: usize) {
        self.trim(limit);
        self.kept.shrink_to_fit();
        self.fifo.shrink_to_fit();
        self.pending.retain(|_, weak| weak.strong_count() > 0);
        self.pending.shrink_to_fit();
    }

    /// Попадание возвращает ячейку по хеш-таблице. При промахе подхватывает активное
    /// чтение или создаёт общую пустую ячейку. Лимит не резервирует память заранее.
    fn at(&mut self, key: K, limit: usize) -> Cell<T> {
        if let Some(cell) = self.kept.get(&key) {
            return cell.clone();
        }
        self.pending.retain(|_, weak| weak.strong_count() > 0);
        let (key, cell) = match self.pending.remove_entry(&key) {
            Some((key, weak)) => (key, weak.upgrade().unwrap_or_default()),
            None => (Arc::new(key), Cell::default()),
        };
        if limit == 0 {
            self.pending.insert(key, Arc::downgrade(&cell));
        } else {
            self.fifo.push_back(key.clone());
            self.kept.insert(key, cell.clone());
            self.trim(limit);
        }
        cell
    }
}

struct Caches {
    scope: Scope,
    last: Option<(Store, u64)>,
    limit: usize,
    frames: Entries<(B256, Roles), Frame>,
    versions: Entries<Version, bool>,
}
impl Caches {
    /// Создаёт пустые очереди для одной области DEX. Ошибок нет.
    fn new(id: &str) -> Self {
        Self {
            scope: Scope::Dex(id.into()), last: None, limit: 0,
            frames: Entries::default(), versions: Entries::default(),
        }
    }

    /// После записи или смены снимка перечитывает точную область. Пустое значение
    /// оставляет только объединение активных чтений. Задержки подписчика на события нет.
    fn refresh(&mut self, store: &Store) {
        let generation = store.generation();
        if self.last.as_ref().is_some_and(|(last, seen)| *seen == generation && store.same(last)) {
            return;
        }
        let value = store.get_exact(LIMIT, &self.scope).and_then(|v| v.as_u64()).unwrap_or_default();
        self.last = Some((store.clone(), generation));
        let limit = usize::try_from(value).unwrap_or(usize::MAX);
        if limit < self.limit {
            self.frames.reduce(limit);
            self.versions.reduce(limit);
        }
        self.limit = limit;
    }
}

#[derive(Default)]
pub(crate) struct Book(Mutex<BTreeMap<&'static str, Caches>>);

impl Book {
    /// Под коротким замком выбирает область и ячейку. Вызовы узла выполняются после выхода.
    fn scoped<R>(&self, ctx: &Ctx, id: &'static str, select: impl FnOnce(&mut Caches, usize) -> R) -> R {
        let mut scopes = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let cache = scopes.entry(id).or_insert_with(|| Caches::new(id));
        cache.refresh(&ctx.settings);
        let limit = cache.limit;
        select(cache, limit)
    }

    /// Возвращает ячейку проверки ролей и ставки по хешу блока. Ошибок нет.
    pub(super) fn frame(&self, ctx: &Ctx, id: &'static str, hash: B256, roles: Roles) -> Cell<Frame> {
        self.scoped(ctx, id, |cache, limit| cache.frames.at((hash, roles), limit))
    }

    /// Возвращает ячейку версии модуля с учётом разрешённых хешей кода. Ошибок нет.
    pub(super) fn version(&self, ctx: &Ctx, id: &'static str, hash: B256, module: Address, allowed: Vec<B256>) -> Cell<bool> {
        self.scoped(ctx, id, |cache, limit| cache.versions.at((hash, module, allowed), limit))
    }
}

#[cfg(test)]
#[path = "motoswap_cache_tests.rs"]
mod tests;
