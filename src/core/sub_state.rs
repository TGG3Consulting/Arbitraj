//! Файл: core/sub_state.rs
//! Что делает: состояние подписок площадок на рыночные данные (для core/subscribe.rs): на что подписана площадка,
//! идёт ли у неё подписка сейчас, последняя причина сбоя. Чистая логика без ожиданий — проверяется тестом.
//! Правила: одна подписка площадки за раз (пересборка не ждёт долгую подписку DEX и не запускает вторую); сбой —
//! одна запись в журнал на причину (та же причина при повторе — без записи); после сбоя повтор — по следующему толчку
//! пересборки (событие рынка, изменение любой настройки — rebuild.rs), а не по кругу.
//! Кто вызывает: core/subscribe.rs, core/rebuild.rs (any_failed).
//! От чего зависит: core/types.
//! Настройки: нет.

use std::collections::{HashMap, HashSet};

use crate::core::types::{Asset, VenueId};

/// Что писать в журнал по итогу подписки.
#[derive(Debug, PartialEq, Eq)]
pub enum Note {
    /// Подписка удалась — запись «Подписка на рыночные данные: N монет».
    Subscribed(usize),
    /// Сбой с новой причиной — запись Warn.
    Failed(String),
    /// Сбой с той же причиной, что и прошлый, — без записи.
    Same,
}

/// Подписки всех площадок.
#[derive(Default)]
pub struct SubState {
    /// На что площадка подписана (после успешной подписки).
    have: HashMap<VenueId, Vec<Asset>>,
    /// Подписка площадки идёт прямо сейчас.
    busy: HashSet<VenueId>,
    /// Последняя причина сбоя подписки площадки (до успеха).
    failed: HashMap<VenueId, String>,
}

impl SubState {
    /// Начать ли подписку площадки на `want`: нет — если подписка уже идёт или площадка подписана ровно на это.
    /// Да — площадка отмечается занятой до done. Ошибок нет.
    pub fn begin(&mut self, venue: &VenueId, want: &[Asset]) -> bool {
        if self.busy.contains(venue) {
            return false;
        }
        let have = self.have.get(venue).map(|v| v.as_slice()).unwrap_or(&[]);
        let subscribed = self.have.contains_key(venue) || want.is_empty();
        if subscribed && have == want && !self.failed.contains_key(venue) {
            return false;
        }
        self.busy.insert(venue.clone());
        true
    }

    /// Итог подписки площадки на `want`. Принимает: площадку, список, Ok или текст сбоя.
    /// Возвращает: что писать в журнал. Ошибок нет.
    pub fn done(&mut self, venue: &VenueId, want: Vec<Asset>, res: Result<(), String>) -> Note {
        self.busy.remove(venue);
        match res {
            Ok(()) => {
                let n = want.len();
                self.have.insert(venue.clone(), want);
                self.failed.remove(venue);
                Note::Subscribed(n)
            }
            Err(why) => {
                self.have.remove(venue);
                if self.failed.get(venue) == Some(&why) {
                    Note::Same
                } else {
                    self.failed.insert(venue.clone(), why.clone());
                    Note::Failed(why)
                }
            }
        }
    }

    /// Есть ли площадка, чья подписка не удалась (тогда любое изменение настроек — повод повторить). Ошибок нет.
    pub fn any_failed(&self) -> bool {
        !self.failed.is_empty()
    }
}
