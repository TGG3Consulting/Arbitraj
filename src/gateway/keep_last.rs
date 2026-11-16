//! Файл: gateway/keep_last.rs
//! Что делает: решения 321/324 — набор «ждут доставки» для событий, не вошедших в канал ядра (чистая логика над
//! настоящим каналом tokio, без площадки). Перенесён из venues/bsc_dex/pools/pending_events.rs (решение 324) и обобщён:
//! по ключу хранится только ПОСЛЕДНЕЕ (слот сливает новое с ждущим — Slot::merge), порядок — очередь ключей (FIFO,
//! честно для всех классов: кто раньше отложен — раньше доставлен; повторное событие ждущего ключа место в очереди не
//! теряет и не занимает новое); срочное (Slot::urgent — «недостоверно») — в начало очереди и не теряется никогда.
//! - offer: ключ уже ждёт — слить (Offer::Replaced). Иначе: у класса ключа ждут отложенные (не срочные) — за ними без
//!   чтения настройки (решение 321: выключение действует, когда класс опустел); очередь не пуста из-за других классов —
//!   `keep()` (настройка): да — за ждущими, нет — как раньше: без ожидания в канал, полон — Offer::Lost; очередь пуста —
//!   без ожидания (try_reserve), полон — `keep()`: да — в очередь (Offer::Deferred), нет — Offer::Lost. Срочное —
//!   без ожидания, полон — в начало очереди без чтения настройки.
//! - deliver: задача-доставщик (одна на набор; запускает вызывающий по Deferred { start: true }). Ждёт место в канале
//!   (Sender::reserve — без опроса), под замком набора снимает первый ключ и отправляет его сообщение тем же
//!   разрешением — между снятием и отправкой нет await, новое событие того же ключа не обгонит старое. Слот может
//!   отдать сообщение и остаток (Take::Send(_, Some)) — остаток ждёт в начале очереди (отправится следующим). Набор
//!   пуст — завершается; канал закрыт — набор очищается. Guard (решение 324, ревью 321): паника или снятие задачи —
//!   признак «доставщик работает» сбрасывается, следующее откладывание запустит нового (набор не копится вечно).
//!
//! Кто вызывает: gateway/core_out.rs (общий набор канала ядра); тесты.
//! От чего зависит: tokio::sync::mpsc, gateway/keep_last_state.rs (состояние под замком и его шаги).
//! Настроек нет (откладывать ли — решает вызывающий через keep).

use std::hash::Hash;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;

use super::keep_last_state::State;

/// Ключ набора и его класс (счёт и правило 321 — по классу).
pub trait Keyed: Eq + Hash + Clone {
    /// Класс ключа (например: пулы площадки, стаканы площадки).
    type Class: Eq + Hash + Clone;
    /// Класс этого ключа. Ошибок нет.
    fn class(&self) -> Self::Class;
}

/// Что слот отдаёт доставщику.
pub enum Take<M, S> {
    /// Отправить сообщение; остаток слота (Some) ждёт в начале очереди.
    Send(M, Option<S>),
    /// Не отправлять (устарело — например, пул недостоверен).
    Skip,
}

/// Что хранится по ключу, пока он ждёт.
pub trait Slot: Sized {
    /// Сообщение канала.
    type Msg;
    /// Новое событие того же ключа, пока он ждёт. Возвращает: заменено ли ждавшее (счёт replaced). Ошибок нет.
    fn merge(&mut self, newer: Self) -> bool;
    /// Сообщение для отправки сразу (свежий слот — одно событие). Ошибок нет.
    fn now(self) -> Self::Msg;
    /// Доставщик: что отправить. Ошибок нет.
    fn take(self) -> Take<Self::Msg, Self>;
    /// Срочное: в начало очереди, не теряется. Ошибок нет.
    fn urgent(&self) -> bool;
}

/// Чем кончилось предложение события.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offer {
    /// Ушло в канал сразу.
    Sent,
    /// Отложено; start — доставщика нет, его надо запустить.
    Deferred { start: bool },
    /// Ключ уже ждал — событие слито с ждущим (более свежее заменило прежнее).
    Replaced,
    /// Канал полон, откладывать не велено — потеряно (как до решений 321/324).
    Lost,
    /// Канал закрыт.
    Closed,
}

/// Счёт класса с прошлой записи журнала и сколько ждёт сейчас.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Отложено (новых ключей в наборе).
    pub deferred: u64,
    /// Заменено более свежим событием того же ключа, пока он ждал.
    pub replaced: u64,
    /// Доставлено доставщиком (сообщений).
    pub delivered: u64,
    /// Снято при доставке (Take::Skip).
    pub dropped: u64,
    /// Потеряно: канал полон, откладывать не велено.
    pub lost: u64,
    /// Ждут сейчас (ключей).
    pub waiting: usize,
}

/// Набор «ждут доставки». Дешёво клонируется (общий).
pub struct Pending<K: Keyed, S>(Arc<Mutex<State<K, S>>>);

impl<K: Keyed, S> Clone for Pending<K, S> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<K: Keyed, S> Default for Pending<K, S> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(State::default())))
    }
}

impl<K: Keyed, S: Slot> Pending<K, S> {
    fn lock(&self) -> MutexGuard<'_, State<K, S>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Предлагает событие (см. шапку). Принимает: канал, ключ, слот с событием, «откладывать ли» (настройка; читается
    /// не больше раза и только когда без неё не решить). Возвращает: Offer. Ошибок нет.
    pub fn offer(
        &self,
        out: &Sender<S::Msg>,
        key: K,
        item: S,
        keep: impl FnOnce() -> bool,
    ) -> Offer {
        let mut s = self.lock();
        if s.last.contains_key(&key) {
            return s.merge(key, item);
        }
        let mut keep = Some(keep);
        let mut want = || keep.take().is_some_and(|f| f());
        let urgent = item.urgent();
        let class = key.class();
        if !urgent && (s.held(&class) || (!s.order.is_empty() && want())) {
            return s.defer(key, item);
        }
        match out.try_reserve() {
            Ok(permit) => {
                permit.send(item.now());
                Offer::Sent
            }
            Err(TrySendError::Closed(())) => Offer::Closed,
            Err(TrySendError::Full(())) => {
                if urgent || (s.order.is_empty() && want()) {
                    return s.defer(key, item);
                }
                s.count(&class).lost += 1;
                Offer::Lost
            }
        }
    }

    /// Доставщик (см. шапку). Принимает: канал. Возвращает: true — всё доставлено (набор пуст), false — канал закрыт
    /// (набор очищен). Ошибок нет.
    pub async fn deliver(self, out: Sender<S::Msg>) -> bool {
        let mut guard = Running {
            set: self.clone(),
            armed: true,
        };
        loop {
            if self.lock().stop_if_empty() {
                guard.armed = false;
                return true;
            }
            let Ok(permit) = out.reserve().await else {
                self.lock().clear();
                guard.armed = false;
                return false;
            };
            let mut s = self.lock();
            // Ключи снимает только доставщик — после проверки выше очередь не пуста.
            let Some(key) = s.order.pop_front() else {
                s.running = false;
                guard.armed = false;
                return true;
            };
            let Some(slot) = s.last.remove(&key) else {
                continue;
            };
            s.deliver_one(key, slot, permit);
        }
    }

    /// Забирает счёт класса с прошлой записи (обнуляет; waiting — сколько ждёт сейчас). Ошибок нет.
    pub fn take(&self, class: &K::Class) -> Counts {
        let mut s = self.lock();
        let c = s.count(class);
        let waiting = c.waiting;
        let taken = std::mem::replace(
            c,
            Counts {
                waiting,
                ..Counts::default()
            },
        );
        Counts { waiting, ..taken }
    }

    /// Счёт класса без обнуления. Ошибок нет.
    pub fn peek(&self, class: &K::Class) -> Counts {
        *self.lock().count(class)
    }

    /// Сколько ключей ждёт сейчас (все классы). Ошибок нет.
    pub fn waiting(&self) -> usize {
        self.lock().order.len()
    }
}

/// Признак «доставщик работает» сбрасывается, если задача кончилась не штатно (паника, снятие). Ошибок нет.
struct Running<K: Keyed, S> {
    set: Pending<K, S>,
    armed: bool,
}

impl<K: Keyed, S> Drop for Running<K, S> {
    fn drop(&mut self) {
        if self.armed {
            self.set.0.lock().unwrap_or_else(|p| p.into_inner()).running = false;
        }
    }
}
