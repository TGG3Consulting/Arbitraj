//! Файл: gateway/calc_share.rs
//! Что делает: решение 336 (задача t157) — общий короб одного подбора объёма (все точки одного кандидата) для адаптеров.
//! Ядро ставит короб на работу подбора (core/size_memo.rs: каждый расчёт точки и каждая пачка точек) ТОЛЬКО когда он
//! нужен: точки считаются параллельно (par — size.parallel_points > 1: адаптер уводит счёт с рабочего потока) или
//! повторное использование (reuse — у направления с DEX всегда, решение 363; core/size_curve.rs). Ни того, ни другого — короба нет вовсе, работа идёт как до 336 (ревью t157 Б1).
//! При reuse адаптер кладёт в короб то, что от объёма не зависит, и берёт это на следующих точках того же подбора
//! (адаптер DEX: допуск пулов классики — quote/usable_reuse.rs, пути графа — quote/graph_reuse.rs). Ядро о содержимом не
//! знает: короб — словарь «имя адаптера → значение» (имя — постоянная строка адаптера, без сборки на горячем пути).
//! Финальная котировка и всё вне подбора идут без короба — всегда на свежих данных. Цикл DEX ↔ DEX — без reuse всегда
//! (core/cycle_plan.rs: план цикла строится прямо из точки подбора, финальной котировки у него нет).
//! Короб задаётся на задачу tokio (task_local, как класс срочности urgency.rs); новая задача его не наследует.
//! Кто вызывает: core/size_memo.rs (Share::new, scope), venues/bsc_dex/quote/cpu.rs (current — перенос в поток счёта),
//! venues/bsc_dex/venue.rs (current — цикл DEX ↔ DEX при параллельных точках).
//! От чего зависит: tokio (task_local), std.
//! Настройки: нет (решает вызывающий: size.parallel_points; reuse — по направлению, решение 363).

use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

/// Короб одного подбора объёма.
pub struct Share {
    /// Точки подбора считаются параллельно (size.parallel_points > 1).
    par: bool,
    /// Повторное использование того, что не зависит от объёма (у направления с DEX — решение 363).
    reuse: bool,
    slots: RwLock<HashMap<&'static str, Arc<dyn Any + Send + Sync>>>,
}

impl Share {
    /// Новый пустой короб. Принимает: параллельны ли точки, включено ли повторное использование. Ошибок нет.
    pub fn new(par: bool, reuse: bool) -> Share {
        Share {
            par,
            reuse,
            slots: RwLock::new(HashMap::new()),
        }
    }

    /// Нужен ли короб: точки параллельны или повторное использование включено. Ошибок нет.
    pub fn needed(par: bool, reuse: bool) -> bool {
        par || reuse
    }

    /// Точки подбора считаются параллельно. Ошибок нет.
    pub fn par(&self) -> bool {
        self.par
    }

    /// Повторное использование включено. Ошибок нет.
    pub fn reuse(&self) -> bool {
        self.reuse
    }

    /// Значение по имени; нет (или другой тип) — кладёт новое из `make` (два потока одного подбора получают одно
    /// значение: вставка под записью с повторной проверкой). Принимает: имя, конструктор. Возвращает: значение.
    /// Ошибок нет.
    pub fn get_or_put<T: Any + Send + Sync>(
        &self,
        key: &'static str,
        make: impl FnOnce() -> T,
    ) -> Arc<T> {
        let found = |g: &HashMap<&'static str, Arc<dyn Any + Send + Sync>>| {
            g.get(key).cloned().and_then(|a| a.downcast::<T>().ok())
        };
        if let Some(v) = found(&self.slots.read().unwrap_or_else(|p| p.into_inner())) {
            return v;
        }
        let mut g = self.slots.write().unwrap_or_else(|p| p.into_inner());
        if let Some(v) = found(&g) {
            return v;
        }
        let v = Arc::new(make());
        g.insert(key, v.clone());
        v
    }
}

tokio::task_local! {
    static SHARE: Arc<Share>;
}

/// Выполняет работу с коробом. Принимает: короб, работу. Возвращает: её итог. Ошибок нет.
pub async fn scope<F: Future>(s: Arc<Share>, work: F) -> F::Output {
    SHARE.scope(s, work).await
}

/// Короб текущей задачи; None — работа вне подбора объёма или короб не нужен. Ошибок нет.
pub fn current() -> Option<Arc<Share>> {
    SHARE.try_with(Arc::clone).ok()
}
