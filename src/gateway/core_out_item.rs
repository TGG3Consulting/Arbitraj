//! Файл: gateway/core_out_item.rs
//! Что делает: решение 324 — ключ и слот общего набора канала событий ядра (gateway/core_out.rs) для набора «ждут
//! доставки» (gateway/keep_last.rs). Ключ — перечисление: пул DEX (площадка, PoolRef) или стакан биржи (площадка,
//! тикер монеты площадки); класс ключа — «пулы площадки» / «стаканы площадки» (счёт журнала и правило 321 — по классу).
//! Слот — последнее событие ключа (MarketEvent::Pool / MarketEvent::Book: полное состояние — ядро кладёт его как есть,
//! поэтому последнее заменяет прежние; проверка «ещё годно» пула DEX — Live, одна на площадку в core_out.rs), либо
//! «недостоверно» (MarketEvent::Invalidated стакана биржи) — срочное: в начало очереди, не теряется, снимает ждавший старый кадр того же ключа; кадр, пришедший после него, ждёт в том же слоте и
//! уходит следующим — ядро не получит «старый кадр после Invalidated».
//! Кто вызывает: gateway/core_out.rs, gateway/core_out_book.rs, venues/bsc_dex/pools/emit_pool.rs.
//! От чего зависит: gateway/keep_last.rs (Keyed, Slot, Take), gateway/events.rs, core/types.
//! Настроек нет.

use std::sync::Arc;

use crate::core::types::{PoolRef, VenueId};

use super::events::MarketEvent;
use super::keep_last::{Keyed, Slot, Take};

/// Ключ набора.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// Пул DEX: площадка, пул.
    Pool(VenueId, PoolRef),
    /// Стакан биржи: площадка, тикер монеты площадки (тот же, что в Invalidated).
    Book(VenueId, String),
}

/// Класс ключа.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Class {
    /// Пулы площадки DEX.
    Pool(VenueId),
    /// Стаканы площадки-биржи.
    Book(VenueId),
}

impl Keyed for Key {
    type Class = Class;

    fn class(&self) -> Class {
        match self {
            Key::Pool(v, _) => Class::Pool(v.clone()),
            Key::Book(v, _) => Class::Book(v.clone()),
        }
    }
}

/// Проверка «событие ещё годно» при доставке (true — отправить). Ревью 324 (Б1): одна на площадку DEX в общем наборе
/// (core_out.rs Feed::live), а не клон в каждом слоте. Ошибок нет.
pub type Live = Arc<dyn Fn(&MarketEvent) -> bool + Send + Sync>;

/// Слот ключа.
pub enum Item {
    /// Последнее событие ключа.
    Event(MarketEvent),
    /// «Недостоверно» и кадр, пришедший после него (уйдёт следующим). Кадр — в Box (clippy large_enum_variant: два
    /// MarketEvent в одном варианте; Box выделяется только при слиянии кадра с ждущим «недостоверно» — редкий путь).
    Invalid(MarketEvent, Option<Box<MarketEvent>>),
}

impl Slot for Item {
    type Msg = MarketEvent;

    fn merge(&mut self, newer: Self) -> bool {
        match newer {
            // «Недостоверно» снимает всё ждавшее по ключу: старый кадр ядру больше не нужен.
            Item::Invalid(inv, _) => {
                *self = Item::Invalid(inv, None);
                true
            }
            Item::Event(ev) => match self {
                Item::Event(old) => {
                    *old = ev;
                    true
                }
                Item::Invalid(_, after) => after.replace(Box::new(ev)).is_some(),
            },
        }
    }

    fn now(self) -> MarketEvent {
        match self {
            Item::Event(ev) | Item::Invalid(ev, _) => ev,
        }
    }

    fn take(self) -> Take<MarketEvent, Self> {
        match self {
            Item::Event(ev) => Take::Send(ev, None),
            Item::Invalid(inv, after) => Take::Send(inv, after.map(|b| Item::Event(*b))),
        }
    }

    fn urgent(&self) -> bool {
        matches!(self, Item::Invalid(..))
    }
}
