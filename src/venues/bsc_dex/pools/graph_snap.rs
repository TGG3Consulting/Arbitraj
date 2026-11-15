//! Файл: venues/bsc_dex/pools/graph_snap.rs
//! Что делает: решение 308 (задача t137) — опубликованный снимок графа маршрутов и общее состояние фонового владельца
//! (Bg), одно на реестр (живёт в кэше графа graph_cache.rs).
//! - Снимок неизменяем; публикация — подмена Arc под коротким RwLock на запись; читатель берёт клон Arc (И1, И3 проекта
//!   docs/notes/graph-snapshot-design-2026-11-16.md) — ни замка построения, ни ожидания построения.
//! - Публикует только текущее поколение владельца (generation): после перезапуска сторожем итог зависшего потока выбрасывается.
//! - Здесь же: признак «владелец запущен» (реестр без владельца — замороженный снимок решения 217, тесты — идёт прежним
//!   путём t132), запрос полной пересборки с причиной, канал событий ядру (Telegram), пульс круга для сторожа, причина
//!   отсутствия снимка (не задана настройка и т. п.), счётчики экрана (graph_stats.rs).
//!
//! Кто вызывает: graph_cache.rs (хранит), graph_owner.rs, graph_full.rs, graph_guard.rs, graph_read.rs, graph_alarm.rs,
//! graph_stats.rs, registry.rs (changes), venue.rs (пересборка по кнопке).
//! От чего зависит: graph.rs (Graph), graph_changes.rs, graph_stats.rs, gateway/events.rs (MarketEvent), tokio.
//! Настройки: нет.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use alloy_primitives::Address;
use chrono::{DateTime, Utc};
use tokio::sync::{Notify, mpsc};

use crate::gateway::events::MarketEvent;

use super::graph::Graph;
use super::graph_changes::Changes;
use super::graph_stats::GraphStats;

/// На чём собран снимок: настройки графа (graph_cache.rs graph_settings — текстом), USDT и его decimals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Basis {
    pub settings: Vec<String>,
    pub usdt: Address,
    pub usdt_dec: u8,
}

/// Опубликованный снимок.
pub struct Snapshot {
    pub graph: Arc<Graph>,
    pub basis: Basis,
    /// Версия реестра, прочитанная до drain круга: все записи с версией ≤ её учтены (И4).
    pub version: u64,
    /// Номер публикации (растёт).
    pub seq: u64,
    /// Собран с нуля (true) или накопительно.
    pub full: bool,
    pub at: Instant,
    pub at_utc: DateTime<Utc>,
}

/// Общее состояние фонового владельца графа.
#[derive(Default)]
pub struct Bg {
    pub changes: Changes,
    pub stats: GraphStats,
    /// Журнал без спама и уведомления (graph_alarm.rs).
    pub alarm: super::graph_alarm::Alarm,
    snap: RwLock<Option<Arc<Snapshot>>>,
    started: AtomicBool,
    generation: AtomicU64,
    seq: AtomicU64,
    rebuild: Notify,
    reason: Mutex<Option<String>>,
    out: Mutex<Option<mpsc::Sender<MarketEvent>>>,
    round_since: Mutex<Option<Instant>>,
    why_none: Mutex<Option<String>>,
    /// Ревью №2 (A, B): накопительной карты нет или она не обновляется (идёт полная сборка после перезапуска,
    /// доводка полной сборки) — действующий снимок отдаётся и отставшим не считается (graph_read.rs).
    catching_up: AtomicBool,
}

impl Bg {
    /// Последний опубликованный снимок. Ошибок нет.
    pub fn load(&self) -> Option<Arc<Snapshot>> {
        self.snap.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Номер следующей публикации. Ошибок нет.
    pub fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Опубликовать снимок, если `generation` — текущее поколение владельца; затем отметка «опубликовано» в наборе изменений.
    /// Возвращает: опубликован ли. Ошибок нет.
    pub fn publish(&self, generation: u64, s: Snapshot) -> bool {
        // Ревью №1 (Q): проверка поколения и запись — под одним замком: сторож меняет поколение до запуска нового
        // владельца, итог старого после этой проверки не запишется.
        let mut slot = self.snap.write().unwrap_or_else(|p| p.into_inner());
        if self.generation.load(Ordering::Acquire) != generation {
            return false;
        }
        *slot = Some(Arc::new(s));
        drop(slot);
        *self.lock_why() = None;
        self.changes.published();
        true
    }

    /// Снять снимок (режим выключен, не задана настройка графа): `why` — причина для читателей и экрана. Ошибок нет.
    pub fn clear(&self, why: Option<String>) {
        *self.snap.write().unwrap_or_else(|p| p.into_inner()) = None;
        *self.lock_why() = why;
    }

    /// Почему снимка нет (None — причина не записана: ещё собирается). Ошибок нет.
    pub fn why_none(&self) -> Option<String> {
        self.lock_why().clone()
    }

    /// Отметить запуск владельца. Возвращает: true — первый запуск (владельца ещё не было). Ошибок нет.
    pub fn start_once(&self) -> bool {
        !self.started.swap(true, Ordering::AcqRel)
    }

    /// Запущен ли владелец у этого реестра. Ошибок нет.
    pub fn started(&self) -> bool {
        self.started.load(Ordering::Acquire)
    }

    /// Текущее ли поколение `g` (брошенная сторожем работа видит «нет» и останавливается — ревью №2 B). Ошибок нет.
    pub fn is_current(&self, g: u64) -> bool {
        self.generation.load(Ordering::Acquire) == g
    }

    /// Ревью №2 (A, B): снимок временно не обновляется накопительно (полная сборка без карты, доводка). Ошибок нет.
    pub fn set_catching_up(&self, on: bool) {
        self.catching_up.store(on, Ordering::Release);
    }

    /// Идёт ли догоняющая полная сборка/доводка (см. set_catching_up). Ошибок нет.
    pub fn catching_up(&self) -> bool {
        self.catching_up.load(Ordering::Acquire)
    }

    /// Новое поколение владельца (сторож — перед каждым запуском). Ошибок нет.
    pub fn next_gen(&self) -> u64 {
        // Под замком снимка (как проверка в publish): публикация старого поколения либо уже записана, либо не запишется.
        let _slot = self.snap.write().unwrap_or_else(|p| p.into_inner());
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Запросить полную пересборку с причиной (кнопка, пересборка подписки, запуск). Ошибок нет.
    pub fn request_full(&self, reason: impl Into<String>) {
        *self.reason.lock().unwrap_or_else(|p| p.into_inner()) = Some(reason.into());
        self.rebuild.notify_one();
    }

    /// Забрать запрос полной пересборки. Ошибок нет.
    pub fn take_full(&self) -> Option<String> {
        self.reason.lock().unwrap_or_else(|p| p.into_inner()).take()
    }

    /// Ждать запроса полной пересборки. Ошибок нет.
    pub async fn rebuild_notified(&self) {
        self.rebuild.notified().await;
    }

    /// Канал событий ядру (Parts::pools). Ошибок нет.
    pub fn set_out(&self, out: mpsc::Sender<MarketEvent>) {
        *self.out.lock().unwrap_or_else(|p| p.into_inner()) = Some(out);
    }

    /// Канал событий ядру, если задан. Ошибок нет.
    pub fn out(&self) -> Option<mpsc::Sender<MarketEvent>> {
        self.out.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Круг владельца начался (пульс для сторожа). Ошибок нет.
    pub fn round_begin(&self) {
        *self.round_since.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
    }

    /// Круг закончился. Ошибок нет.
    pub fn round_end(&self) {
        *self.round_since.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    /// Сколько идёт текущий круг (None — круга нет). Ошибок нет.
    pub fn round_age(&self) -> Option<std::time::Duration> {
        self.round_since
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .map(|t| t.elapsed())
    }

    fn lock_why(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.why_none.lock().unwrap_or_else(|p| p.into_inner())
    }
}
