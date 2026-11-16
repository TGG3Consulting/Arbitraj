//! Файл: gateway/keep_last_deliver.rs
//! Что делает: задача-доставщик набора «ждут доставки» (gateway/keep_last.rs Pending::deliver; одна на набор). Ждёт
//! место в канале (Sender::reserve — без опроса), под замком набора снимает первый ключ и его сообщение (остаток слота —
//! Take::Send(_, Some) — ждёт в начале очереди). Сообщению без проверки (Check::needs = false — стаканы бирж) —
//! отправка тем же разрешением под тем же замком: между снятием и отправкой нет await. Сообщению с проверкой (пул DEX —
//! Live читает реестр пулов) — ревью 324, Н4: проверка ВНЕ замка набора (кадры бирж не ждут писателя реестра): ключ
//! помечается «в доставке» (State::flight; busy не уменьшается), замок снимается, проверка, замок снова, отправка тем
//! же разрешением, пометка снимается, busy публикуется. Пока ключ в доставке, его новые события встают за ним
//! (keep_last_decide.rs: waits) — новое событие того же ключа не обгонит проверяемое; другие ключи идут своим путём.
//! Набор пуст — завершается; канал закрыт — набор очищается. Guard: паника (в том числе в проверке) или снятие задачи —
//! признак «доставщик работает» и пометка «в доставке» сбрасываются, busy публикуется (набор не копится вечно, ключ
//! не остаётся «в доставке»).
//! Кто вызывает: gateway/core_out.rs (Feed::spawn); тесты.
//! От чего зависит: gateway/keep_last.rs (Pending, Check, Keyed, Slot), gateway/keep_last_state.rs, tokio::sync::mpsc.
//! Настроек нет.

use tokio::sync::mpsc::Sender;

use super::keep_last::{Check, Keyed, Pending, Slot};

impl<K: Keyed, S: Slot> Pending<K, S> {
    /// Доставщик (см. шапку). Принимает: канал, проверку «ещё годно». Возвращает: true — всё доставлено (набор
    /// пуст), false — канал закрыт (набор очищен). Ошибок нет.
    pub async fn deliver(self, out: Sender<S::Msg>, check: impl Check<K, S::Msg>) -> bool {
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
                let mut s = self.lock();
                s.clear();
                self.publish(&s);
                guard.armed = false;
                return false;
            };
            let mut s = self.lock();
            // Ключи снимает только доставщик — после проверки выше очередь не пуста.
            let Some(key) = s.order.pop_front() else {
                s.running = false;
                self.publish(&s);
                guard.armed = false;
                return true;
            };
            let Some(msg) = s.last.remove(&key).and_then(|e| s.take_one(&key, e)) else {
                self.publish(&s);
                continue;
            };
            if !check.needs(&key) {
                permit.send(msg);
                s.done(&key, true);
                // busy −1 — только после отправки (keep_last.rs, гарантия порядка).
                self.publish(&s);
                continue;
            }
            // Н4: проверка вне замка набора; ключ «в доставке» держит busy и свои новые события за собой.
            s.flight = Some(key.clone());
            self.publish(&s);
            drop(s);
            let live = check.live(&key, &msg);
            let mut s = self.lock();
            if live {
                permit.send(msg);
            }
            s.done(&key, live);
            s.flight = None;
            self.publish(&s);
        }
    }
}

/// Признак «доставщик работает» и пометка «в доставке» сбрасываются, если задача кончилась не штатно (паника, снятие).
struct Running<K: Keyed, S: Slot> {
    set: Pending<K, S>,
    armed: bool,
}

impl<K: Keyed, S: Slot> Drop for Running<K, S> {
    fn drop(&mut self) {
        if self.armed {
            let mut s = self.set.lock();
            s.running = false;
            s.flight = None;
            self.set.publish(&s);
        }
    }
}
