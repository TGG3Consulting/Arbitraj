//! Файл: gateway/keep_last_state.rs
//! Что делает: состояние набора «ждут доставки» (gateway/keep_last.rs) под замком и его шаги без ожидания: слить с
//! ждущим, отложить (срочное — в начало), правило 321 по классу (held), доставить один ключ разрешением канала,
//! очистить при закрытом канале. Отдельный файл — чтобы keep_last.rs держал только порядок offer/deliver.
//! Правило 321 (held) считает только слоты, отложенные НАСТРОЙКОЙ: срочные (Invalidated) и «вынужденные» (остаток
//! срочного слота после его доставки — ревью 324, Б3) — не в счёт: без настройки класс не уходит в режим «откладывать».
//! Кто вызывает: gateway/keep_last.rs.
//! От чего зависит: gateway/keep_last.rs (Keyed, Slot, Take, Offer, Counts), tokio::sync::mpsc (Permit).
//! Настроек нет.

use std::collections::{HashMap, VecDeque};

use tokio::sync::mpsc::Permit;

use super::keep_last::{Counts, Keyed, Offer, Slot, Take};

/// Состояние набора под замком.
pub(super) struct State<K: Keyed, S> {
    /// Очередь ключей (порядок доставки).
    pub(super) order: VecDeque<K>,
    /// Слот каждого ждущего ключа.
    pub(super) last: HashMap<K, Entry<S>>,
    /// Доставщик запущен.
    pub(super) running: bool,
    /// Счёт по классам.
    counts: HashMap<K::Class, Counts>,
    /// Ждущих слотов класса, отложенных НЕ настройкой (срочные и вынужденные — Entry::exempt).
    exempt: HashMap<K::Class, usize>,
}

/// Слот ждущего ключа и признак «вынужденный».
pub(super) struct Entry<S> {
    pub(super) slot: S,
    /// Отложен без настройки (остаток срочного слота после его доставки — ревью 324, Б3).
    forced: bool,
}

impl<S: Slot> Entry<S> {
    /// Отложен не настройкой (срочный или вынужденный) — не держит класс по правилу 321. Ошибок нет.
    fn exempt(&self) -> bool {
        self.forced || self.slot.urgent()
    }
}

impl<K: Keyed, S> Default for State<K, S> {
    fn default() -> Self {
        Self {
            order: VecDeque::new(),
            last: HashMap::new(),
            running: false,
            counts: HashMap::new(),
            exempt: HashMap::new(),
        }
    }
}

impl<K: Keyed, S: Slot> State<K, S> {
    /// Счёт класса (создаётся пустым). Ошибок нет.
    pub(super) fn count(&mut self, class: &K::Class) -> &mut Counts {
        self.counts.entry(class.clone()).or_default()
    }

    /// Правило 321 по классу: у класса ждут отложенные настройкой (не срочные и не вынужденные) — новое встаёт за ними
    /// без чтения настройки. Ошибок нет.
    pub(super) fn held(&self, class: &K::Class) -> bool {
        let waiting = self.counts.get(class).map_or(0, |c| c.waiting);
        let exempt = self.exempt.get(class).copied().unwrap_or(0);
        waiting > exempt
    }

    /// Ключ уже ждёт — слить новое событие с его слотом; срочное — ключ в начало очереди. Ошибок нет.
    pub(super) fn merge(&mut self, key: K, item: S) -> Offer {
        let class = key.class();
        let Some(e) = self.last.get_mut(&key) else {
            return Offer::Replaced;
        };
        let (was, was_urgent) = (e.exempt(), e.slot.urgent());
        let replaced = e.slot.merge(item);
        let (now, now_urgent) = (e.exempt(), e.slot.urgent());
        self.exempt_delta(&class, was, now);
        if now_urgent && !was_urgent {
            self.raise(&key);
        }
        if replaced {
            self.count(&class).replaced += 1;
        }
        Offer::Replaced
    }

    /// Новый ключ в набор: срочное — в начало, остальное — в конец. Возвращает: Deferred (start — запустить
    /// доставщика). Ошибок нет.
    pub(super) fn defer(&mut self, key: K, item: S) -> Offer {
        let class = key.class();
        let urgent = item.urgent();
        if urgent {
            self.order.push_front(key.clone());
        } else {
            self.order.push_back(key.clone());
        }
        self.last.insert(
            key,
            Entry {
                slot: item,
                forced: false,
            },
        );
        self.exempt_delta(&class, false, urgent);
        let c = self.count(&class);
        c.deferred += 1;
        c.waiting += 1;
        let start = !self.running;
        self.running = true;
        Offer::Deferred { start }
    }

    /// Доставщик: отправляет сообщение слота разрешением канала (под замком — без await между снятием и отправкой);
    /// остаток слота — в начало очереди «вынужденным» (Б3: без настройки, правило 321 его не считает). Принимает: ключ
    /// (уже снят с очереди), слот, разрешение. Ошибок нет.
    pub(super) fn deliver_one(&mut self, key: K, e: Entry<S>, permit: Permit<'_, S::Msg>) {
        let class = key.class();
        // Ключ уже снят: счёт «ждут» уменьшается ДО слота — паника в Slot::take не оставит ложного «класс ждёт».
        self.exempt_delta(&class, e.exempt(), false);
        self.gone(&class);
        match e.slot.take() {
            Take::Send(msg, rest) => {
                permit.send(msg);
                self.count(&class).delivered += 1;
                if let Some(rest) = rest {
                    self.exempt_delta(&class, false, true);
                    self.count(&class).waiting += 1;
                    self.order.push_front(key.clone());
                    let e = Entry {
                        slot: rest,
                        forced: true,
                    };
                    self.last.insert(key, e);
                }
            }
            Take::Skip => self.count(&class).dropped += 1,
        }
    }

    /// Очередь пуста — доставщик останавливается (признак снимается под замком). Возвращает: пуста ли. Ошибок нет.
    pub(super) fn stop_if_empty(&mut self) -> bool {
        if self.order.is_empty() {
            self.running = false;
            return true;
        }
        false
    }

    /// Канал закрыт: набор очищается, доставщик останавливается. Ошибок нет.
    pub(super) fn clear(&mut self) {
        self.order.clear();
        self.last.clear();
        self.exempt.clear();
        for c in self.counts.values_mut() {
            c.waiting = 0;
        }
        self.running = false;
    }

    /// Ключ покинул набор: ждущих класса на одного меньше. Ошибок нет.
    fn gone(&mut self, class: &K::Class) {
        let c = self.count(class);
        c.waiting = c.waiting.saturating_sub(1);
    }

    /// Ждущих класса, отложенных не настройкой: было / стало. Ошибок нет.
    fn exempt_delta(&mut self, class: &K::Class, was: bool, now: bool) {
        if was == now {
            return;
        }
        let n = self.exempt.entry(class.clone()).or_default();
        *n = if now { *n + 1 } else { n.saturating_sub(1) };
    }

    /// Ждущий ключ — в начало очереди. Ошибок нет.
    fn raise(&mut self, key: &K) {
        if let Some(i) = self.order.iter().position(|k| k == key)
            && let Some(k) = self.order.remove(i)
        {
            self.order.push_front(k);
        }
    }
}
