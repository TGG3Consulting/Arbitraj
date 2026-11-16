//! Файл: gateway/keep_last_decide.rs
//! Что делает: решение медленного пути набора «ждут доставки» (gateway/keep_last.rs slow) под замком: ключ уже ждёт —
//! слить; класс держит правило 321 или ждут другие и настройка «да» — отложить; иначе без ожидания в канал, полон —
//! срочное или (очередь пуста и настройка «да») — отложить, иначе потеряно. Отдельный файл — чтобы keep_last.rs и
//! keep_last_state.rs держались в 200 строк (ревью 324, Н6).
//! Кто вызывает: gateway/keep_last.rs (Pending::slow).
//! От чего зависит: gateway/keep_last_state.rs (State), gateway/keep_last.rs (Ask, Keyed, Offer, Slot), tokio.
//! Настроек нет (откладывать ли — Ask вызывающего).

use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;

use super::keep_last::{Ask, Keyed, Offer, Slot};
use super::keep_last_state::State;

impl<K: Keyed, S: Slot> State<K, S> {
    /// Решение медленного пути (см. шапку). Принимает: канал, ключ, слот, «откладывать ли» (читается не больше раза и
    /// только когда без неё не решить). Возвращает: Offer. Ошибок нет.
    pub(super) fn decide(
        &mut self,
        out: &Sender<S::Msg>,
        key: K,
        item: S,
        keep: &mut Ask<impl FnOnce() -> bool>,
    ) -> Offer {
        if self.last.contains_key(&key) {
            return self.merge(key, item);
        }
        let urgent = item.urgent();
        if !urgent && (self.held(&key.class()) || (!self.order.is_empty() && keep.get())) {
            return self.defer(key, item);
        }
        match out.try_reserve() {
            Ok(permit) => {
                permit.send(item.now());
                Offer::Sent
            }
            Err(TrySendError::Closed(())) => Offer::Closed,
            Err(TrySendError::Full(())) => {
                if urgent || (self.order.is_empty() && keep.get()) {
                    return self.defer(key, item);
                }
                Offer::Lost
            }
        }
    }
}
