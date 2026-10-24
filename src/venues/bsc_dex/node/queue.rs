//! Файл: venues/bsc_dex/node/queue.rs
//! Что делает: очередь мест к узлу BSC со срочностью (решение 61). Место — время `step` (темп считает node/pace.rs).
//! Освободившееся место получает первый ждущий старшего класса (gateway/urgency.rs: Trade → Check → Quote →
//! Background), внутри класса — по приходу; строгий порядок подтверждён Тиграном. Чтобы фон (загрузка пулов, цикл
//! квотера) не голодал, у него гарантированная доля мест — настройка bsc.node_background_share_pct (передаётся сюда
//! числом p): пока фон ждёт, каждое место, отданное не фону, добавляет фону p / (100 − p) «права»; набралось целое —
//! следующее место идёт фону. Так при постоянном ожидании фон получает p % мест. Нет доли (не задана, 0) — строгий
//! порядок. Отменённый запрос (drop) уходит из очереди. Пауза всей очереди (429) — hold.
//! Без классов (reserve) — очередь индексатора (directory/limit.rs).
//! Кто вызывает: node/pace.rs (take), directory/limit.rs (reserve, hold), tests/bsc_dex_node_priority.rs.
//! От чего зависит: gateway/urgency.rs, tokio (Notify, время).
//! Настройки: нет своих (доля и темп приходят от pace.rs).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;

use crate::gateway::urgency::Urgency;

/// Общая очередь запросов к узлу HTTP. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Pace {
    state: Arc<Mutex<State>>,
    /// Будит ждущих: место отдано, ждущий ушёл, пауза очереди изменилась.
    wake: Arc<Notify>,
}

#[derive(Default)]
struct State {
    /// Когда может уйти следующий запрос. None — сразу.
    next: Option<Instant>,
    /// Ждущие места: (класс, номер прихода). Первый — следующий на отправку (если не очередь доли фона).
    waiting: BTreeSet<(Urgency, u64)>,
    /// Номер следующего пришедшего.
    seq: u64,
    /// Накопленное право фона на место (доля фона).
    credit: f64,
}

impl State {
    /// Кто получит следующее место: фон, если его право набралось и он ждёт; иначе первый по порядку.
    fn chosen(&self) -> Option<(Urgency, u64)> {
        let bg = self.waiting.iter().find(|(c, _)| *c == Urgency::Background);
        match bg {
            Some(k) if self.credit >= 1.0 => Some(*k),
            _ => self.waiting.first().copied(),
        }
    }

    /// Учёт отданного места для доли фона. Принимает: кому отдано, долю фона (%). Ошибок нет.
    fn account(&mut self, given: Urgency, share: f64) {
        if given == Urgency::Background {
            self.credit = (self.credit - 1.0).max(0.0);
        } else if share > 0.0 && self.waiting.iter().any(|(c, _)| *c == Urgency::Background) {
            self.credit += if share >= 100.0 {
                1.0
            } else {
                share / (100.0 - share)
            };
        }
    }
}

/// Место ждущего в очереди; уходит из очереди при отмене запроса (drop) — чтобы не задерживать других.
struct Ticket<'a> {
    pace: &'a Pace,
    key: (Urgency, u64),
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        if self.pace.lock().waiting.remove(&self.key) {
            self.pace.wake.notify_waiters();
        }
    }
}

impl Pace {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Занимает место в очереди на время `step` без классов (одна очередь индексатора, directory/limit.rs).
    /// Возвращает: когда отправлять. Ошибок нет.
    pub(crate) fn reserve(&self, step: Duration) -> Instant {
        let mut st = self.lock();
        let now = Instant::now();
        let slot = st.next.map_or(now, |n| n.max(now));
        st.next = Some(slot + step);
        slot
    }

    /// Вся очередь ждёт не меньше `pause` от текущего момента. Ошибок нет.
    pub(crate) fn hold(&self, pause: Duration) {
        let mut st = self.lock();
        let until = Instant::now() + pause;
        st.next = Some(st.next.map_or(until, |n| n.max(until)));
        drop(st);
        self.wake.notify_waiters();
    }

    /// Ждёт своего места с классом `class` и занимает его на `step` (см. шапку). Принимает: класс, время места,
    /// долю фона в процентах (0 — строгий порядок). Ошибок нет.
    pub async fn take(&self, class: Urgency, step: Duration, share: f64) {
        let ticket = {
            let mut st = self.lock();
            st.seq += 1;
            let key = (class, st.seq);
            st.waiting.insert(key);
            Ticket { pace: self, key }
        };
        loop {
            // Подписка до проверки: пробуждение между проверкой и ожиданием не теряется (Notify::notified).
            let woke = self.wake.notified();
            let wait_until = {
                let mut st = self.lock();
                let now = Instant::now();
                if st.chosen() != Some(ticket.key) {
                    None
                } else if st.next.is_none_or(|n| n <= now) {
                    st.waiting.remove(&ticket.key);
                    st.account(class, share);
                    st.next = Some(now + step);
                    drop(st);
                    self.wake.notify_waiters();
                    return;
                } else {
                    st.next
                }
            };
            match wait_until {
                Some(at) => {
                    tokio::select! {
                        _ = tokio::time::sleep_until(at) => {}
                        _ = woke => {}
                    }
                }
                None => woke.await,
            }
        }
    }
}
