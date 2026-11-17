//! Файл: venues/bsc_dex/quote/shallow_memo.rs
//! Что делает: решение 393 (повторное ревью t178, Н-B2, Н-B1), решение 403 (задача t188 и её ревью, Н1) — память отбора
//! мелких пулов пары (quote/shallow_pick.rs):
//! - снимок допуска (предел dex.shallow_pools_per_pair на момент сборки и допущенные пулы по парам) — для пути
//!   котировки (pools/graph.rs, pools/graph_eval.rs, quote/select.rs); его пишет только построитель
//!   (quote/shallow_build.rs) и ТОЛЬКО когда допуск или предел сменились; читается копией `Arc` из поля потока,
//!   перечитывается раз на публикацию; без общего замка, без чтения настроек и без копий;
//! - отпечатки входов по парам (quote/shallow_stamps.rs) — для путей вне котировки (shallow_pick.rs pick), меняются
//!   каждым проходом построителя только у пересчитанных пар;
//! - изменённые пары (quote/shallow_dirty.rs) — их отмечает реестр после записи, когда предел задан.
//!
//! Решение 403: раньше память была одна на весь реестр под общим замком с ключом «версия реестра» — версия растёт на
//! каждом обмене любого пула, память сбрасывалась почти на каждом вызове, и 48 потоков расчёта стояли в futex
//! (17.11.2026, сборка 4656c113, dex.shallow_pools_per_pair = 5: system CPU 35–40 %).
//!
//! Хранится в реестре (pools/registry.rs поле shallow, `Arc` — общий у клонов реестра); у замороженного снимка —
//! снимок допуска живого на момент заморозки (pools/registry_frozen.rs, построителя нет).
//! Кто вызывает: quote/shallow_pick.rs (with, stamps), quote/shallow_build.rs (publish, take_dirty, notified),
//! pools/registry.rs (wrote, wrote_all), pools/registry_frozen.rs (fixed, load).
//! От чего зависит: quote/shallow_stamps.rs, quote/shallow_dirty.rs, core/types (PoolRef), tokio (Notify).
//! Настройки: нет.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use tokio::sync::Notify;

use crate::core::types::PoolRef;

pub use super::shallow_stamps::{Pair, Slot, Stamp, Stamps};

use super::shallow_dirty::Dirty;

/// Снимок допуска: предел на момент сборки (0 — мелкие не допускаются) и допущенные пулы пар (пары без допущенных
/// пулов не хранятся).
#[derive(Debug, Clone, Default)]
pub struct Snap {
    pub limit: usize,
    pub sel: HashMap<Pair, Arc<Vec<PoolRef>>>,
}

impl Snap {
    /// Допущен ли пул `r` пары `pair` (предел > 0 и пул в отборе). Ошибок нет.
    pub fn admits(&self, pair: &Pair, r: &PoolRef) -> bool {
        self.limit > 0 && self.sel.get(pair).is_some_and(|s| s.contains(r))
    }

    /// Все допущенные пулы (при пределе 0 — ни одного). Ошибок нет.
    pub fn visible(&self) -> HashSet<PoolRef> {
        if self.limit == 0 {
            return HashSet::new();
        }
        self.sel.values().flat_map(|v| v.iter().cloned()).collect()
    }
}

/// Общая часть памяти.
struct Shared {
    /// Номер памяти (отличает память живого реестра от памяти замороженного снимка в поле потока).
    id: u64,
    /// Номер публикации снимка допуска: растёт после каждой замены.
    seq: AtomicU64,
    snap: RwLock<Arc<Snap>>,
    stamps: Stamps,
    dirty: Dirty,
    /// Отмечать ли записи реестра и будить построителя (предел задан).
    on: AtomicBool,
    /// Построитель запущен (или его не будет — замороженный снимок).
    started: AtomicBool,
    wake: Notify,
}

/// Память отбора. Дешёво клонируется (общая у клонов реестра).
#[derive(Clone)]
pub struct Memo(Arc<Shared>);

/// Номера памятей.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// Последний прочитанный этим потоком снимок допуска: (номер памяти, номер публикации, снимок).
    static SEEN: RefCell<Option<(u64, u64, Arc<Snap>)>> = const { RefCell::new(None) };
}

impl Default for Memo {
    fn default() -> Self {
        Memo::make(Arc::default(), false)
    }
}

impl Memo {
    /// Память с данным снимком; `fixed` — построителя не будет. Ошибок нет.
    fn make(snap: Arc<Snap>, fixed: bool) -> Memo {
        Memo(Arc::new(Shared {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            seq: AtomicU64::new(0),
            snap: RwLock::new(snap),
            stamps: Stamps::default(),
            dirty: Dirty::default(),
            on: AtomicBool::new(false),
            started: AtomicBool::new(fixed),
            wake: Notify::new(),
        }))
    }

    /// Память замороженного снимка реестра: снимок допуска живого на момент заморозки, без построителя. Ошибок нет.
    pub fn fixed(snap: Arc<Snap>) -> Memo {
        Memo::make(snap, true)
    }

    /// Текущий снимок допуска (копия `Arc`; для построителя и заморозки). Ошибок нет.
    pub fn load(&self) -> Arc<Snap> {
        self.0
            .snap
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Номер публикации снимка допуска (растёт только при смене допуска или предела). Ошибок нет.
    pub fn seq(&self) -> u64 {
        self.0.seq.load(Ordering::Acquire)
    }

    /// Чтение снимка допуска на горячем пути: снимок из поля потока, замок публикации берётся, только когда номер
    /// публикации сменился (раз на публикацию на поток); счётчик ссылок общего `Arc` на попадании не трогается. `f` не
    /// должна снова звать `with`. Принимает: функцию над снимком. Возвращает: её итог. Ошибок нет.
    pub fn with<R>(&self, f: impl FnOnce(&Snap) -> R) -> R {
        let seq = self.0.seq.load(Ordering::Acquire);
        SEEN.with(|c| {
            {
                let mut m = c.borrow_mut();
                let hit = matches!(m.as_ref(), Some((id, s, _)) if *id == self.0.id && *s == seq);
                if !hit {
                    *m = Some((self.0.id, seq, self.load()));
                }
            }
            let m = c.borrow();
            let (_, _, snap) = m.as_ref().expect("снимок положен строкой выше");
            f(snap.as_ref())
        })
    }

    /// Публикация нового снимка допуска (только построитель). Ошибок нет.
    pub fn publish(&self, s: Snap) {
        *self.0.snap.write().unwrap_or_else(|p| p.into_inner()) = Arc::new(s);
        self.0.seq.fetch_add(1, Ordering::AcqRel);
    }

    /// Отпечатки входов по парам (пути вне котировки). Ошибок нет.
    pub fn stamps(&self) -> &Stamps {
        &self.0.stamps
    }

    /// Запись в реестр пулов пар `pairs`: отметить их и разбудить построителя, если предел задан (иначе — одно
    /// атомарное чтение). Ошибок нет.
    pub fn wrote(&self, pairs: &[Pair]) {
        if self.0.on.load(Ordering::Acquire) {
            self.0.dirty.mark(pairs);
            self.0.wake.notify_one();
        }
    }

    /// Пулы удалены (retain): нужен полный проход, если предел задан. Ошибок нет.
    pub fn wrote_all(&self) {
        if self.0.on.load(Ordering::Acquire) {
            self.0.dirty.mark_full();
            self.0.wake.notify_one();
        }
    }

    /// Забрать изменённые пары: (пары, нужен ли полный проход). Ошибок нет.
    pub fn take_dirty(&self) -> (HashSet<Pair>, bool) {
        self.0.dirty.take()
    }

    /// Разбудить построителя независимо от предела (пересборка подписки). Ошибок нет.
    pub fn wake_now(&self) {
        self.0.wake.notify_one();
    }

    /// Отмечать ли записи реестра (построитель — по пределу). Ошибок нет.
    pub fn set_on(&self, on: bool) {
        self.0.on.store(on, Ordering::Release);
    }

    /// Ожидание пробуждения построителя. Ошибок нет.
    pub fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.0.wake.notified()
    }

    /// true — построитель ещё не запущен и запускается вызывающим (один раз). Ошибок нет.
    pub fn start_once(&self) -> bool {
        !self.0.started.swap(true, Ordering::AcqRel)
    }
}
