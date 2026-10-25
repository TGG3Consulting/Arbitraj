//! Файл: venues/bsc_dex/node/receipts_pipe.rs
//! Что делает: конвейер ленты квитанций (решение 69): следующие блоки запрашиваются параллельно — не больше
//! bsc.receipts_ahead_blocks вперёд от последнего разобранного, а забираются строго по порядку номеров. Блок, который
//! не пришёл (сбой узла), запрашивается снова, уже пришедшие следующие ждут его и не запрашиваются повторно. Реорг или
//! обрыв — всё, что в пути, отменяется. Каждый запрос — своя задача tokio с классом срочности ленты (urgency::scope):
//! класс задачи не наследуется, его передают явно.
//! Кто вызывает: node/receipts_task.rs; tests/bsc_dex_receipts_pipe.rs.
//! От чего зависит: tokio, gateway/urgency.rs.
//! Настройки: нет (окно опережения даёт вызывающий из bsc.receipts_ahead_blocks).

use std::collections::BTreeMap;
use std::future::Future;

use tokio::task::JoinHandle;

use crate::gateway::urgency;

/// Блоки в пути: номер → задача запроса.
pub struct Pipe<T> {
    inflight: BTreeMap<u64, JoinHandle<T>>,
}

impl<T> Default for Pipe<T> {
    fn default() -> Self {
        Pipe {
            inflight: BTreeMap::new(),
        }
    }
}

impl<T: Send + 'static> Pipe<T> {
    /// Запускает запросы блоков `from` … min(`to`, `from` + `ahead` − 1), которых ещё нет в пути. Принимает: первый
    /// нужный блок, голову, окно опережения (0 — как 1: хотя бы следующий блок), запрос блока. Ошибок нет.
    pub fn fill<F, Fut>(&mut self, from: u64, to: u64, ahead: u64, fetch: F)
    where
        F: Fn(u64) -> Fut,
        Fut: Future<Output = T> + Send + 'static,
    {
        if to < from {
            return;
        }
        let last = to.min(from.saturating_add(ahead.max(1) - 1));
        let class = urgency::current();
        for n in from..=last {
            self.inflight
                .entry(n)
                .or_insert_with(|| tokio::spawn(urgency::scope(class, fetch(n))));
        }
    }

    /// Ждёт блок `n` (по порядку: следующие остаются в пути). Возвращает: итог запроса; None — блока `n` нет в пути
    /// или задачу отменили. Ошибок нет.
    pub async fn take(&mut self, n: u64) -> Option<T> {
        let h = self.inflight.remove(&n)?;
        h.await.ok()
    }

    /// Сколько блоков в пути. Ошибок нет.
    pub fn len(&self) -> usize {
        self.inflight.len()
    }

    /// В пути ничего нет. Ошибок нет.
    pub fn is_empty(&self) -> bool {
        self.inflight.is_empty()
    }

    /// Отменяет всё, что в пути (реорг, обрыв, смена источника). Ошибок нет.
    pub fn clear(&mut self) {
        for (_, h) in std::mem::take(&mut self.inflight) {
            h.abort();
        }
    }
}

impl<T> Drop for Pipe<T> {
    /// Запросы в пути отменяются вместе с конвейером. Ошибок нет.
    fn drop(&mut self) {
        for (_, h) in std::mem::take(&mut self.inflight) {
            h.abort();
        }
    }
}
