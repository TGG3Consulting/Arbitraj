//! Файл: gateway/core_out_must_queue.rs
//! Что делает: решение 333, ревью — очередь «ждут места» одного канала событий ядра (одна на канал, core_out.rs
//! Feed::must). Две очереди по порядку (FIFO): Money и Info; доставщик (gateway/core_out_must_deliver.rs) ждёт место
//! в канале и отдаёт СНАЧАЛА Money, затем Info (ревью, Н1: «остановлена» не стоит за сотнями уведомлений).
//! - Money: очередь рода пуста — try_reserve; канал полон — в очередь ВСЕГДА (Б1, без настройки).
//! - Info: очередь рода пуста — try_reserve; полон — в очередь, если «откладывать» (venue.info_events_keep = да), иначе
//!   потеря. Очередь Info не пуста — новое встаёт в конец при любой настройке (порядок). Предел venue.info_events_max
//!   (Н2): достигнут — потеря со счётом; не задан — без предела.
//! - Денежное событие ждало места дольше venue.money_events_alert_ms (Н9) — после его доставки в очередь Money
//!   ставится уведомление venue.money_event_delayed (Telegram — notify.event.*; не теряется, как Money).
//! - Ревью, Н3: доставщик упал (паника, снятие задачи) — признак «работает» сброшен (core_out_must_deliver.rs Running):
//!   следующее сообщение запустит нового (start = !running), очередь не застревает навсегда.
//! - Канал закрыт — очереди очищаются (слать некому), счёт «сброшено» — в журнал.
//!
//! Под замком нет await; чтение настроек (keep, max) под замком — синхронное (память хранилища).
//! Кто вызывает: gateway/core_out_must.rs (put), gateway/core_out_must_deliver.rs (State), gateway/core_out_must_log.rs
//! (snapshot), core_out.rs (Default).
//! От чего зависит: gateway/core_out_must_log.rs (журнал, уведомление о задержке), gateway/lost_log.rs (Lost), tokio.
//! Настройки: venue.info_events_keep, venue.info_events_max — чтения передаёт вызывающий (core_out_must.rs).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;

use super::core_out_must::Kind;
use super::core_out_must_log as log;
use super::events::MarketEvent;
use super::lost_log::Lost;

/// Сообщение в очереди и когда оно в неё встало.
#[derive(Debug)]
pub(super) struct Item {
    pub ev: MarketEvent,
    pub at: Instant,
}

/// Состояние под замком.
#[derive(Debug, Default)]
pub(super) struct State {
    pub money: VecDeque<Item>,
    pub info: VecDeque<Item>,
    /// Доставщик запущен.
    pub running: bool,
}

/// Счёт с прошлой записи журнала (атомарный; обнуляет запись — core_out_must_log.rs).
#[derive(Debug, Default)]
pub(super) struct Counts {
    pub deferred_money: AtomicU64,
    pub deferred_info: AtomicU64,
    pub delivered_money: AtomicU64,
    pub delivered_info: AtomicU64,
    pub lost_info: AtomicU64,
    /// Сброшено при закрытом канале.
    pub dropped: AtomicU64,
    /// Дольше всех ждало денежное, мс.
    pub max_wait_money_ms: AtomicU64,
}

/// Очередь «ждут места» одного канала ядра.
#[derive(Debug, Default)]
pub struct Queue {
    pub(super) state: Mutex<State>,
    pub(super) counts: Counts,
    /// Потери рода Info / Money (Money — только без рантайма tokio).
    pub(super) lost_info: Lost,
    pub(super) lost_money: Lost,
    /// Часы и номер записи журнала счёта (core_out_must_log.rs note).
    pub(super) notes: log::Notes,
}

/// Глубина очереди сейчас.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub money: usize,
    pub info: usize,
    pub running: bool,
}

/// Итог put без потери.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Put {
    Sent,
    Closed,
    /// В очереди; start — запустить доставщика.
    Kept {
        start: bool,
    },
}

/// Почему потеряно.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Info: канал полон, venue.info_events_keep не «да».
    NotKept,
    /// Info: очередь достигла venue.info_events_max.
    Max(u64),
    /// Нужен доставщик, а рантайма tokio нет.
    NoRuntime,
}

/// Замок без паники на отравлении. Ошибок нет.
pub(super) fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Queue {
    /// Глубина очереди и признак доставщика. Ошибок нет.
    pub fn snapshot(&self) -> Snapshot {
        let s = lock(&self.state);
        Snapshot {
            money: s.money.len(),
            info: s.info.len(),
            running: s.running,
        }
    }

    /// Решение по одному сообщению (см. шапку). Принимает: канал, сообщение, род, есть ли рантайм, «откладывать ли
    /// Info» и предел Info (читаются, только когда нужны). Возвращает: Put или (почему, сообщение) — потеряно.
    /// Ошибок нет.
    pub(super) fn put(
        &self,
        out: &Sender<MarketEvent>,
        ev: MarketEvent,
        kind: Kind,
        rt: bool,
        keep: impl FnOnce() -> bool,
        max: impl FnOnce() -> Option<u64>,
    ) -> Result<Put, Box<(Why, MarketEvent)>> {
        let mut s = lock(&self.state);
        let busy = match kind {
            Kind::Money => !s.money.is_empty(),
            Kind::Info => !s.info.is_empty(),
        };
        if !busy {
            match out.try_reserve() {
                Ok(p) => {
                    p.send(ev);
                    return Ok(Put::Sent);
                }
                Err(TrySendError::Closed(())) => return Ok(Put::Closed),
                Err(TrySendError::Full(())) => {}
            }
            if kind == Kind::Info && !keep() {
                return Err(Box::new((Why::NotKept, ev)));
            }
        }
        if kind == Kind::Info
            && let Some(m) = max()
            && s.info.len() as u64 >= m
        {
            return Err(Box::new((Why::Max(m), ev)));
        }
        if !s.running && !rt {
            return Err(Box::new((Why::NoRuntime, ev)));
        }
        let item = Item {
            ev,
            at: Instant::now(),
        };
        match kind {
            Kind::Money => {
                s.money.push_back(item);
                self.counts.deferred_money.fetch_add(1, Ordering::Relaxed);
            }
            Kind::Info => {
                s.info.push_back(item);
                self.counts.deferred_info.fetch_add(1, Ordering::Relaxed);
            }
        }
        let start = !s.running;
        s.running = true;
        Ok(Put::Kept { start })
    }
}
