//! Файл: settings/snapshot.rs
//! Что делает: решение 409 (t194) — значения настроек в памяти как неизменяемый снимок за ArcSwap. Читатели
//! (Store::get_exact, all, pinned — горячий путь котировки, графа, сигналов) берут снимок без замка (`load`): раньше
//! каждое чтение брало RwLock::read, и под нагрузкой (perf 18.11: read_contended ← Store::get_exact) потоки бились за
//! счётчик читателей замка. Запись (Store::set, Store::mark_now) — по одной под своим замком писателя: текущий снимок →
//! копия → изменение → новый снимок (`store`). Читатель видит либо старый снимок целиком, либо новый целиком.
//! Снимки решения 239 (pinned.rs) — свой Snapshot с тем же Arc значений (копия значений не делается).
//! Кто вызывает: settings/store.rs, settings/store_mark.rs, settings/pinned.rs.
//! От чего зависит: arc-swap.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use arc_swap::{ArcSwap, Guard};
use serde_json::Value;

use super::value::Scope;

/// Ключ значения: имя настройки и область.
pub type Key = (String, Scope);
/// Все заданные значения.
pub type Values = HashMap<Key, Value>;

/// Значения настроек в памяти: снимок для читателей и замок для писателей.
pub struct Snapshot {
    /// Текущий снимок значений — читается без замка.
    now: ArcSwap<Values>,
    /// Писатели — строго по одному (чтение текущего значения и новый снимок без гонки между set и mark_now).
    write: Mutex<()>,
}

impl Snapshot {
    /// Что делает: снимок из готовых значений. Принимает: значения. Возвращает: Snapshot. Ошибок нет.
    pub fn new(values: Arc<Values>) -> Self {
        Self {
            now: ArcSwap::new(values),
            write: Mutex::new(()),
        }
    }

    /// Что делает: текущий снимок для короткого чтения (без замка). Возвращает: охрану со ссылкой на значения.
    /// Ошибок нет.
    pub fn load(&self) -> Guard<Arc<Values>> {
        self.now.load()
    }

    /// Что делает: текущий снимок как Arc (для копии на момент входа, решение 239). Ошибок нет.
    pub fn load_full(&self) -> Arc<Values> {
        self.now.load_full()
    }

    /// Что делает: изменение под замком писателя. f получает текущие значения и возвращает (результат, новые значения
    /// или None — не менять). Новые значения публикуются одним снимком; after вызывается под тем же замком сразу после
    /// публикации (поколение решения 352 растёт вместе со значением). Принимает: f, after. Возвращает: результат f.
    /// Ошибок нет (отравленный замок — продолжаем: под ним нет данных, только очередь писателей).
    pub fn update<R>(
        &self,
        f: impl FnOnce(&Arc<Values>) -> (R, Option<Values>),
        after: impl FnOnce(),
    ) -> R {
        let _turn = self.write.lock().unwrap_or_else(|p| p.into_inner());
        let cur = self.now.load_full();
        let (r, next) = f(&cur);
        if let Some(next) = next {
            self.now.store(Arc::new(next));
            after();
        }
        r
    }
}
