//! Файл: gateway/calc_share.rs
//! Что делает: решение 336 (задача t157) — общий короб одного подбора объёма (все точки одного кандидата) для адаптеров.
//! Ядро ставит короб на работу подбора (core/size_memo.rs: каждый расчёт точки и каждая пачка точек), адаптер по
//! своей настройке может положить туда то, что от объёма не зависит, и взять это на следующих точках того же подбора
//! (адаптер DEX: допуск путей графа — quote/graph_reuse.rs, допуск пулов классики — quote/usable_reuse.rs;
//! dex.quote_reuse). Ядро о содержимом не знает: короб — это
//! словарь «ключ адаптера → значение» плюс признак «точки считаются параллельно» (par, size.parallel_points > 1) — по
//! нему адаптер уводит свой счёт с рабочего потока (иначе одновременные точки шли бы одним потоком).
//! Финальная котировка и всё вне подбора идут без короба — всегда на свежих данных.
//! Короб задаётся на задачу tokio (task_local, как класс срочности urgency.rs); новая задача его не наследует.
//! Кто вызывает: core/size_memo.rs (scope), venues/bsc_dex/quote/cpu.rs (current — перенос в поток счёта),
//! venues/bsc_dex/venue.rs (current — цикл DEX ↔ DEX при параллельных точках).
//! От чего зависит: tokio (task_local), std.
//! Настройки: нет (значения кладут адаптеры по своим настройкам).

use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

/// Короб одного подбора объёма.
pub struct Share {
    /// Точки подбора считаются параллельно (size.parallel_points > 1).
    par: bool,
    slots: Mutex<HashMap<String, Arc<dyn Any + Send + Sync>>>,
}

impl Share {
    /// Новый пустой короб. Принимает: считаются ли точки параллельно. Ошибок нет.
    pub fn new(par: bool) -> Share {
        Share {
            par,
            slots: Mutex::new(HashMap::new()),
        }
    }

    /// Точки подбора считаются параллельно. Ошибок нет.
    pub fn par(&self) -> bool {
        self.par
    }

    /// Значение по ключу. Принимает: ключ. Возвращает: значение нужного типа или None (нет или другой тип). Ошибок нет.
    pub fn get<T: Any + Send + Sync>(&self, key: &str) -> Option<Arc<T>> {
        let g = self.slots.lock().unwrap_or_else(|p| p.into_inner());
        g.get(key).cloned().and_then(|a| a.downcast::<T>().ok())
    }

    /// Значение по ключу; нет (или другой тип) — кладёт новое из `make` под тем же замком (два потока одного подбора
    /// получают одно значение). Принимает: ключ, конструктор. Возвращает: значение. Ошибок нет.
    pub fn get_or_put<T: Any + Send + Sync>(&self, key: &str, make: impl FnOnce() -> T) -> Arc<T> {
        let mut g = self.slots.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(v) = g.get(key).cloned().and_then(|a| a.downcast::<T>().ok()) {
            return v;
        }
        let v = Arc::new(make());
        g.insert(key.to_string(), v.clone());
        v
    }

    /// Кладёт значение по ключу (прежнее заменяется). Принимает: ключ, значение. Ошибок нет.
    pub fn put<T: Any + Send + Sync>(&self, key: String, v: Arc<T>) {
        let mut g = self.slots.lock().unwrap_or_else(|p| p.into_inner());
        g.insert(key, v);
    }
}

tokio::task_local! {
    static SHARE: Arc<Share>;
}

/// Выполняет работу с коробом. Принимает: короб, работу. Возвращает: её итог. Ошибок нет.
pub async fn scope<F: Future>(s: Arc<Share>, work: F) -> F::Output {
    SHARE.scope(s, work).await
}

/// Короб текущей задачи; None — работа вне подбора объёма. Ошибок нет.
pub fn current() -> Option<Arc<Share>> {
    SHARE.try_with(Arc::clone).ok()
}
