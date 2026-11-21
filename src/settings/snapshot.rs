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

/// Все заданные значения: имя настройки → область → значение. Решение 409 (п. д): поиск по (&str, &Scope) без
/// выделения строки ключа (раньше get_exact собирал ключ (name.to_string(), scope.clone()) на каждое чтение).
/// Ревью t194 (М1): значения одного имени — за Arc: изменение копирует карту имён (указатели) и значения только этого
/// имени (Arc::make_mut), а не все значения — импорт настроек (api/settings_io.rs, set по одной) больше не копирует
/// всё на каждую запись. Пустых внутренних карт нет (put убирает их) — обход и «есть ли область» те же, что у плоской
/// карты.
#[derive(Clone, Default)]
pub struct Values(HashMap<String, Arc<HashMap<Scope, Value>>>);

impl Values {
    /// Что делает: значение ровно в этой области. Принимает: имя, область. Возвращает: ссылку или None. Ошибок нет.
    pub fn get(&self, name: &str, scope: &Scope) -> Option<&Value> {
        self.0.get(name).and_then(|m| m.get(scope))
    }

    /// Что делает: задаёт (Some) или убирает (None) значение. Принимает: имя, область, значение. Ошибок нет.
    pub fn put(&mut self, name: &str, scope: &Scope, value: Option<Value>) {
        match value {
            Some(v) => {
                if !self.0.contains_key(name) {
                    self.0.insert(name.to_string(), Arc::default());
                }
                if let Some(m) = self.0.get_mut(name) {
                    Arc::make_mut(m).insert(scope.clone(), v);
                }
            }
            None => {
                let empty = match self.0.get_mut(name) {
                    Some(m) if m.contains_key(scope) => {
                        let inner = Arc::make_mut(m);
                        inner.remove(scope);
                        inner.is_empty()
                    }
                    _ => false,
                };
                if empty {
                    self.0.remove(name);
                }
            }
        }
    }

    /// Что делает: все значения (имя, область, значение). Ошибок нет.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Scope, &Value)> {
        self.0
            .iter()
            .flat_map(|(n, m)| m.iter().map(move |(s, v)| (n.as_str(), s, v)))
    }

    /// Что делает: есть ли хоть одно значение в этой области. Ошибок нет.
    pub fn has_scope(&self, scope: &Scope) -> bool {
        self.0.values().any(|m| m.contains_key(scope))
    }
}

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
        f: impl FnOnce(&Values) -> (R, Option<Values>),
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
