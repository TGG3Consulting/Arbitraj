//! Файл: venues/bsc_dex/pools/graph_changes.rs
//! Что делает: решение 308 (задача t137) — набор пулов, изменённых в реестре с последнего круга фонового владельца
//! графа (graph_owner.rs), и отставание снимка графа от реестра.
//! - mark — реестр (registry.rs put / update / retain) отмечает пул ПОД СВОИМ ЗАМКОМ ЗАПИСИ И ДО увеличения версии
//!   (инвариант И4 проекта docs/notes/graph-snapshot-design-2026-11-16.md): владелец, прочитавший версию V до drain,
//!   получает все отметки записей с версией ≤ V. Отметка будит владельца (Notify::notify_one хранит одно разрешение —
//!   всплеск записей даёт одно пробуждение).
//! - Это множество, не очередь: размер ≤ числа пулов реестра, переполнения нет — всплеск любой длины = один круг.
//! - Отставание (lag) — сколько времени назад случилась самая старая запись реестра, ещё не вошедшая в опубликованный
//!   снимок; None — снимок учитывает все записи. Предохранитель (graph_read.rs) и сторож (graph_guard.rs) смотрят его.
//! - Слежение выключено (dex.graph_background не «да») — mark стоит одно атомарное чтение, набор пуст.
//!
//! Кто вызывает: registry.rs (mark, mark_all), graph_owner.rs / graph_full.rs (drain, published, track), graph_read.rs
//! и graph_guard.rs (lag, notified, poke), graph_stats.rs (pending).
//! От чего зависит: core/types (PoolRef), tokio (Notify).
//! Настройки: нет.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use crate::core::types::PoolRef;

/// Изменённые пулы и отметки времени. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Changes {
    inner: Arc<Mutex<Inner>>,
    on: Arc<AtomicBool>,
    wake: Arc<Notify>,
}

/// Состояние под замком.
#[derive(Default)]
struct Inner {
    /// Пулы, изменённые после последнего drain.
    set: HashSet<PoolRef>,
    /// Когда отмечен первый пул текущего набора.
    pending_since: Option<Instant>,
    /// Самая старая запись, ещё не вошедшая в опубликованный снимок.
    unpublished_since: Option<Instant>,
}

impl Changes {
    /// Включить/выключить слежение (владелец — по dex.graph_background). Выключение очищает набор. Ошибок нет.
    pub fn track(&self, on: bool) {
        self.on.store(on, Ordering::Release);
        if !on {
            *self.lock() = Inner::default();
        }
    }

    /// Идёт ли слежение. Ошибок нет.
    pub fn tracking(&self) -> bool {
        self.on.load(Ordering::Acquire)
    }

    /// Пул изменился (вызывается под замком записи реестра, до роста версии). Ошибок нет.
    pub fn mark(&self, r: &PoolRef) {
        if !self.tracking() {
            return;
        }
        {
            let mut m = self.lock();
            let now = Instant::now();
            m.set.insert(r.clone());
            m.pending_since.get_or_insert(now);
            m.unpublished_since.get_or_insert(now);
        }
        self.wake.notify_one();
    }

    /// Несколько пулов (retain — удалённые). Ошибок нет.
    pub fn mark_all(&self, refs: &[PoolRef]) {
        for r in refs {
            self.mark(r);
        }
    }

    /// Вернуть в набор пулы, забранные кругом или доводкой, которые не удалось применить (ревью №1 N, №2 E) —
    /// следующий круг их повторит. `since` — когда они были отмечены впервые (из drain): отставание при публикации
    /// считается от него, а не от возврата (повторное ревью е). Ошибок нет.
    pub fn remark(&self, refs: impl IntoIterator<Item = PoolRef>, since: Option<Instant>) {
        if !self.tracking() {
            return;
        }
        let mut any = false;
        {
            let mut m = self.lock();
            for r in refs {
                m.set.insert(r);
                any = true;
            }
            if any {
                let t = since.unwrap_or_else(Instant::now);
                m.pending_since = Some(m.pending_since.map_or(t, |p| p.min(t)));
                m.unpublished_since = Some(m.unpublished_since.map_or(t, |p| p.min(t)));
            }
        }
        if any {
            self.wake.notify_one();
        }
    }

    /// Забрать набор (только владелец). Возвращает: пулы, изменённые после прошлого drain, и когда отмечен самый
    /// старый из них (None — набор пуст) — для отставания в момент публикации. Ошибок нет.
    pub fn drain(&self) -> (HashSet<PoolRef>, Option<Instant>) {
        let mut m = self.lock();
        let since = m.pending_since.take();
        (std::mem::take(&mut m.set), since)
    }

    /// Снимок, собранный по последнему drain, опубликован: неопубликованными остаются только записи после drain.
    /// Ошибок нет.
    pub fn published(&self) {
        let mut m = self.lock();
        m.unpublished_since = m.pending_since;
    }

    /// Отставание снимка: возраст самой старой неопубликованной записи; None — отставания нет. Ошибок нет.
    pub fn lag(&self) -> Option<Duration> {
        self.lock().unpublished_since.map(|t| t.elapsed())
    }

    /// Сколько пулов ждут следующего круга. Ошибок нет.
    pub fn pending(&self) -> usize {
        self.lock().set.len()
    }

    /// Ждать пробуждения (отметка или poke). Ошибок нет.
    pub async fn notified(&self) {
        self.wake.notified().await;
    }

    /// Разбудить владельца без отметки (читатель увидел снимок на других настройках). Ошибок нет.
    pub fn poke(&self) {
        self.wake.notify_one();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}
