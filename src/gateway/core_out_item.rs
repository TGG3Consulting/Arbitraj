//! Файл: gateway/core_out_item.rs
//! Что делает: решение 324 — ключ и слот общего набора канала событий ядра (gateway/core_out.rs) для набора «ждут
//! доставки» (gateway/keep_last.rs). Ключ — перечисление: пул DEX (площадка, PoolRef) или стакан биржи (площадка,
//! тикер монеты площадки); класс ключа — «пулы площадки» / «стаканы площадки» (счёт журнала и правило 321 — по классу).
//! Слот — последнее событие ключа (MarketEvent::Pool / MarketEvent::Book: полное состояние — ядро кладёт его как есть,
//! поэтому последнее заменяет прежние) с необязательной проверкой «ещё годно» при доставке (Live: пул DEX снят или
//! недостоверен — не отправляется), либо «недостоверно» (MarketEvent::Invalidated стакана биржи) — срочное: в начало
//! очереди, не теряется, снимает ждавший старый кадр того же ключа; кадр, пришедший после него, ждёт в том же слоте и
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

/// Проверка «событие ещё годно» при доставке (true — отправить). Ошибок нет.
pub type Live = Arc<dyn Fn(&MarketEvent) -> bool + Send + Sync>;

/// Слот ключа.
pub enum Item {
    /// Последнее событие ключа и проверка при доставке (None — без проверки).
    Event(MarketEvent, Option<Live>),
    /// «Недостоверно» и кадр, пришедший после него (уйдёт следующим).
    Invalid(MarketEvent, Option<(MarketEvent, Option<Live>)>),
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
            Item::Event(ev, live) => match self {
                Item::Event(old, old_live) => {
                    *old = ev;
                    *old_live = live;
                    true
                }
                Item::Invalid(_, after) => after.replace((ev, live)).is_some(),
            },
        }
    }

    fn now(self) -> MarketEvent {
        match self {
            Item::Event(ev, _) | Item::Invalid(ev, _) => ev,
        }
    }

    fn take(self) -> Take<MarketEvent, Self> {
        match self {
            Item::Event(ev, live) => {
                if live.as_ref().is_none_or(|f| f(&ev)) {
                    Take::Send(ev, None)
                } else {
                    Take::Skip
                }
            }
            Item::Invalid(inv, after) => Take::Send(inv, after.map(|(e, l)| Item::Event(e, l))),
        }
    }

    fn urgent(&self) -> bool {
        matches!(self, Item::Invalid(..))
    }
}
