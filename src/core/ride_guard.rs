//! Файл: core/ride_guard.rs
//! Что делает: решение 394 (ревью 3 t179, Н5) — место сделки по монете снимается при ЛЮБОМ конце задачи исполнителя,
//! в том числе при панике внутри run: замок целиком или своя запись в идущих (coin_ride::release) и группа докупки
//! (coin_group: dropped — вложенная не дошла до учёта покупки; released — запись снята, проверка «последняя ли» и
//! уборка пула). Было: вызовы стояли после run — паника оставляла монету занятой до перезапуска, а отложенные USDT
//! группы ждали перезапуска. Теперь — в Drop; обычный конец — те же вызовы в том же порядке.
//! Кто вызывает: executor.rs (accepted — владеет; run — пишет boarded при coin_ride::board).
//! От чего зависит: coin_ride.rs, coin_group.rs, coin_key.rs, engine.rs.
//! Настройки: нет.

use super::coin_key::CoinKey;
use super::coin_ride::Seat;
use super::engine::Core;

/// Место новой сделки по монете (coin_ride::Seat) и ID сделки после board.
pub(super) struct Slot {
    pub(super) seat: Seat,
    pub(super) boarded: Option<String>,
}

/// Страж места сделки: снимает его в Drop.
pub(super) struct RideGuard {
    pub(super) core: Core,
    pub(super) key: CoinKey,
    pub(super) slot: Slot,
}

impl Drop for RideGuard {
    /// Что делает: снять замок целиком или свою запись в идущих; у группы докупки — dropped / released. Ошибок нет
    /// (общий возврат — отдельной задачей; без рантайма — не ставится, coin_group.rs spawn_if_last).
    fn drop(&mut self) {
        let (core, key) = (&self.core, &self.key);
        super::coin_ride::release(core, key, self.slot.boarded.as_deref());
        match (&self.slot.seat, &self.slot.boarded) {
            (Seat::Join(g), None) => super::coin_group::dropped(core, key, g),
            (Seat::Join(g), Some(_)) => super::coin_group::released(core, key, g),
            (Seat::Lead, Some(id)) => super::coin_group::released(core, key, id),
            _ => {}
        }
    }
}
