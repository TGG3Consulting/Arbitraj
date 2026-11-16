//! Файл: venues/bsc_dex/pools/pending_events.rs
//! Что делает: решение 321 — набор «ждут доставки» для событий, не вошедших в канал ядра (чистая логика над настоящим
//! каналом tokio, без площадки; пул — pools/emit_pool.rs). Ключ — пул; по ключу хранится только ПОСЛЕДНЕЕ событие
//! (MarketEvent::Pool несёт состояние пула — ядро кладёт его в market_state как есть, core/market_state.rs apply), так что
//! набор не больше числа пулов и без дублей. Порядок — очередь ключей (кто раньше отложен — раньше доставлен).
//! - offer: очередь пуста — отправка без ожидания (try_reserve); канал полон — `keep()` (настройка читается только
//!   здесь): нет — событие потеряно, как раньше (Offer::Lost); да — ключ в очередь (Offer::Deferred). Очередь не пуста —
//!   событие сразу за ждущими (сначала ждущие); ключ уже ждёт — событие заменяет его прежнее (Offer::Replaced).
//! - deliver: задача-доставщик (одна на набор; запускает вызывающий по Deferred { start: true }). Ждёт место в канале
//!   (Sender::reserve — разрешение семафора канала, без опроса: проснётся, когда ядро заберёт событие, даже если новых
//!   событий больше нет), под замком набора берёт первый ключ и отправляет его последнее событие тем же разрешением —
//!   между снятием из набора и отправкой нет await, поэтому новое событие того же пула не обгонит старое. Событие пула,
//!   который уже недостоверен или снят (`live` = нет), не отправляется (снято). Набор пуст — задача завершается;
//!   канал закрыт — набор очищается, задача завершается.
//!
//! Кто вызывает: pools/emit_pool.rs (offer, deliver, take), pools/lost_events.rs (take — для записи журнала).
//! От чего зависит: tokio::sync::mpsc.
//! Настроек нет (включает вызывающий — dex.pool_events_keep, решение 321).

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;

/// Чем кончилось предложение события.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offer {
    /// Ушло в канал сразу.
    Sent,
    /// Отложено; start — доставщика нет, его надо запустить.
    Deferred { start: bool },
    /// Пул уже ждал — его событие заменено этим (более свежим).
    Replaced,
    /// Канал полон, откладывать не велено — потеряно (как до решения 321).
    Lost,
    /// Канал закрыт.
    Closed,
}

/// Счёт с прошлой записи журнала и сколько ждёт сейчас.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Отложено (новых ключей в наборе).
    pub deferred: u64,
    /// Заменено более свежим событием того же пула, пока он ждал.
    pub replaced: u64,
    /// Доставлено доставщиком.
    pub delivered: u64,
    /// Снято при доставке: пул недостоверен или снят с подписки.
    pub dropped: u64,
    /// Ждут сейчас.
    pub waiting: usize,
}

/// Состояние набора под замком.
struct State<K, T> {
    order: VecDeque<K>,
    last: HashMap<K, T>,
    running: bool,
    counts: Counts,
}

/// Набор «ждут доставки». Дешёво клонируется (общий).
pub struct Pending<K, T>(Arc<Mutex<State<K, T>>>);

impl<K, T> Clone for Pending<K, T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<K, T> Default for Pending<K, T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(State {
            order: VecDeque::new(),
            last: HashMap::new(),
            running: false,
            counts: Counts::default(),
        })))
    }
}

impl<K: Eq + Hash + Clone, T> Pending<K, T> {
    fn lock(&self) -> MutexGuard<'_, State<K, T>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Предлагает событие (см. шапку). Принимает: канал, ключ, событие, обёртку в сообщение канала, «откладывать ли»
    /// (вызывается только при полном канале и пустой очереди). Возвращает: Offer. Ошибок нет.
    pub fn offer<M>(
        &self,
        out: &Sender<M>,
        key: K,
        item: T,
        wrap: impl FnOnce(T) -> M,
        keep: impl FnOnce() -> bool,
    ) -> Offer {
        let mut s = self.lock();
        if s.order.is_empty() {
            match out.try_reserve() {
                Ok(permit) => {
                    permit.send(wrap(item));
                    return Offer::Sent;
                }
                Err(TrySendError::Closed(())) => return Offer::Closed,
                Err(TrySendError::Full(())) => {
                    if !keep() {
                        return Offer::Lost;
                    }
                }
            }
        } else if let Some(slot) = s.last.get_mut(&key) {
            *slot = item;
            s.counts.replaced += 1;
            return Offer::Replaced;
        }
        s.order.push_back(key.clone());
        s.last.insert(key, item);
        s.counts.deferred += 1;
        let start = !s.running;
        s.running = true;
        Offer::Deferred { start }
    }

    /// Доставщик (см. шапку). Принимает: канал, обёртку, «пул ещё живой» (ключ → да/нет). Возвращает: true — всё
    /// доставлено (набор пуст), false — канал закрыт (набор очищен). Ошибок нет.
    pub async fn deliver<M>(
        self,
        out: Sender<M>,
        wrap: impl Fn(T) -> M,
        live: impl Fn(&K) -> bool,
    ) -> bool {
        loop {
            {
                let mut s = self.lock();
                if s.order.is_empty() {
                    s.running = false;
                    return true;
                }
            }
            let Ok(permit) = out.reserve().await else {
                let mut s = self.lock();
                s.order.clear();
                s.last.clear();
                s.running = false;
                return false;
            };
            let mut s = self.lock();
            // Ключи снимает только доставщик — после проверки выше очередь не пуста.
            let Some(key) = s.order.pop_front() else {
                s.running = false;
                return true;
            };
            match s.last.remove(&key) {
                Some(item) if live(&key) => {
                    permit.send(wrap(item));
                    s.counts.delivered += 1;
                }
                _ => s.counts.dropped += 1,
            }
        }
    }

    /// Забирает счёт с прошлой записи (обнуляет; waiting — сколько ждёт сейчас). Ошибок нет.
    pub fn take(&self) -> Counts {
        let mut s = self.lock();
        let waiting = s.order.len();
        let c = std::mem::take(&mut s.counts);
        Counts { waiting, ..c }
    }

    /// Сколько ждёт сейчас. Ошибок нет.
    pub fn waiting(&self) -> usize {
        self.lock().order.len()
    }
}
