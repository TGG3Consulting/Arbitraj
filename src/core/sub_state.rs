//! Файл: core/sub_state.rs
//! Что делает: состояние подписок площадок на рыночные данные (для core/subscribe.rs): на что подписана площадка,
//! идёт ли у неё подписка сейчас, последняя причина сбоя. Чистая логика без ожиданий — проверяется тестом.
//! Правила: одна подписка площадки за раз (пересборка не ждёт долгую подписку DEX и не запускает вторую); сбой —
//! одна запись в журнал на причину (та же причина при повторе — без записи); после сбоя повтор — по следующему толчку
//! пересборки (событие рынка, изменение любой настройки — rebuild.rs), а не по кругу.
//! Решение 196: reset — забыть подписку площадки (поток молчит при включении биржи или «Проверить подключение» —
//! следующая пересборка подпишет заново); held — выключенная площадка, чья подписка держится до закрытия её сделок.
//! Кто вызывает: core/subscribe.rs, core/rebuild.rs (any_failed), core/feed_live.rs (reset), core/feed_hold.rs (held).
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
    /// Решение 196: выключенные площадки, чья подписка держится до закрытия идущих сделок.
    held: HashSet<VenueId>,
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

    /// Решение 196: забыть подписку площадки (на что подписана и причину сбоя), чтобы следующая пересборка подписала
    /// её заново тем же порядком, что при запуске. Идущую подписку не трогает. Возвращает: была ли подписка. Ошибок нет.
    pub fn reset(&mut self, venue: &VenueId) -> bool {
        self.failed.remove(venue);
        self.have.remove(venue).is_some_and(|v| !v.is_empty())
    }

    /// На что площадка подписана сейчас (None — подписки не было или сброшена). Ошибок нет.
    pub fn have(&self, venue: &VenueId) -> Option<&[Asset]> {
        self.have.get(venue).map(|v| v.as_slice())
    }

    /// Решение 196: отметить, держится ли подписка выключенной площадки. Возвращает: изменилось ли (было держится —
    /// стало нет и наоборот), для одной записи в журнал на переход. Ошибок нет.
    pub fn hold(&mut self, venue: &VenueId, on: bool) -> bool {
        if on {
            self.held.insert(venue.clone())
        } else {
            self.held.remove(venue)
        }
    }

    /// Держится ли подписка выключенной площадки (решение 196). Ошибок нет.
    pub fn is_held(&self, venue: &VenueId) -> bool {
        self.held.contains(venue)
    }

    /// Есть ли площадка, чья подписка не удалась (тогда любое изменение настроек — повод повторить). Ошибок нет.
    pub fn any_failed(&self) -> bool {
        !self.failed.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(t: &str) -> Asset {
        Asset {
            ticker: t.into(),
            bsc_contract: None,
        }
    }

    /// Решение 196: подписка на тот же список не повторяется, после reset — повторяется; hold — переходы.
    #[test]
    fn reset_forces_resubscribe_and_hold_transitions() {
        let v = VenueId("bitget".into());
        let mut s = SubState::default();
        assert!(s.begin(&v, &[a("RWA")]));
        s.done(&v, vec![a("RWA")], Ok(()));
        assert!(!s.begin(&v, &[a("RWA")]), "тот же список — без подписки");
        assert!(s.reset(&v), "подписка была");
        assert!(s.have(&v).is_none());
        assert!(s.begin(&v, &[a("RWA")]), "после reset — подписка заново");
        s.done(&v, vec![a("RWA")], Ok(()));
        assert!(s.hold(&v, true));
        assert!(!s.hold(&v, true), "уже держится");
        assert!(s.hold(&v, false));
        assert!(!s.hold(&v, false));
    }
}
