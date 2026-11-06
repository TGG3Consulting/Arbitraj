//! Файл: core/feed_live.rs
//! Что делает: решение 196 (Тигран, 06.11.2026: «включаю биржу — если веб-сокеты не подняты, трафика нет, делается
//! подписка, та же логика, что при старте бота; ввёл креды после старта — не перезапускать бота») — живость потока
//! рынка площадки и подъём подписки без перезапуска.
//! Живость: время последнего события рынка площадки (стакан биржи, пул DEX — core/tick.rs отмечает в Seen).
//! Подъём (revive): площадку включили (venue.enabled нет → да) или нажали «Проверить подключение»
//! (POST /api/check/venue/{id}). Подписка есть и событие было не раньше feed.stale_after_ms назад — поток живой,
//! подписка не тронута.
//! Нет (тишина или подписки не было) — подписка площадки забывается (sub_state.rs reset) и пересборка подписывает её
//! заново тем же порядком, что при запуске (rebuild.rs → subscribe.rs); справочник сетей бирж перечитывается сразу
//! (после ввода ключа не ждать опроса venue.status_poll_ms / venue.networks_poll_ms). Итог — в журнал (core.feed).
//! Выключение (venue.enabled да → нет) — запись о том, держится ли поток до закрытия сделок (feed_hold.rs).
//! Сохранение того же значения изменением настройки не считается (settings/store.rs) — для «включена, но потока нет»
//! есть «Проверить подключение».
//! Кто вызывает: core/tick.rs (mark), core/rebuild.rs (on_setting), api/check.rs (Core::revive_feed).
//! От чего зависит: engine.rs, sub_state.rs, feed_hold.rs, registry_sync.rs (enabled), settings.
//! Настройки: feed.stale_after_ms (по площадке), venue.enabled.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::core::types::VenueId;
use crate::settings::live::Change;
use crate::settings::read::At;
use crate::settings::scope::Scope;
use crate::storage::journal::Level;

use super::engine::{Core, lk};

/// Время последнего события рынка по площадке.
#[derive(Default)]
pub struct Seen(Mutex<HashMap<VenueId, Instant>>);

impl Seen {
    /// Отметить событие рынка площадки (сейчас). Ошибок нет.
    pub fn mark(&self, venue: &VenueId) {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match m.get_mut(venue) {
            Some(t) => *t = Instant::now(),
            None => {
                m.insert(venue.clone(), Instant::now());
            }
        }
    }

    /// Сколько прошло с последнего события площадки. None — событий не было. Ошибок нет.
    pub fn age(&self, venue: &VenueId) -> Option<Duration> {
        let m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        m.get(venue).map(Instant::elapsed)
    }
}

impl Core {
    /// Решение 196: проверить поток рынка площадки и, если он молчит или подписки нет, поднять подписку заново.
    /// Принимает: площадку, повод (для журнала). Возвращает: итог одной строкой (для журнала и ответа кнопки).
    /// Ошибок нет: не задана feed.stale_after_ms — итог с причиной, подписка не трогается.
    pub async fn revive_feed(&self, venue: &VenueId, why: &str) -> String {
        let at = At {
            venue: Some(&venue.0),
            coin: None,
        };
        let deps = &self.inner.deps;
        let (level, msg) = match deps.settings.integer("feed.stale_after_ms", at) {
            Err(e) => (
                Level::Warn,
                format!("{why}: поток рынка не проверен — {}", e.message),
            ),
            Ok(stale) => {
                let age = self.inner.feed_seen.age(venue);
                // Живой — свежее событие И подписка есть (выключенная площадка могла отписаться секунды назад).
                let subscribed = lk(&self.inner.subs)
                    .have(venue)
                    .is_some_and(|h| !h.is_empty());
                if subscribed && age.is_some_and(|a| a.as_millis() as u64 <= stale) {
                    let ms = age.map(|a| a.as_millis()).unwrap_or_default();
                    (
                        Level::Info,
                        format!(
                            "{why}: поток рынка живой (последнее событие {ms} мс назад) — подписка не тронута"
                        ),
                    )
                } else {
                    let had = lk(&self.inner.subs).reset(venue);
                    self.inner.kick_networks.notify_waiters();
                    self.request_rebuild();
                    let state = match (had, age) {
                        (false, _) => "подписки не было".to_string(),
                        (true, Some(a)) => format!("поток молчал {} мс", a.as_millis()),
                        (true, None) => "событий рынка с запуска не было".to_string(),
                    };
                    (
                        Level::Info,
                        format!(
                            "{why}: подписка поднимается заново ({state}; feed.stale_after_ms = {stale})"
                        ),
                    )
                }
            }
        };
        deps.log(level, "core.feed", Some(&venue.0), msg.clone(), None)
            .await;
        msg
    }
}

/// Изменение настройки: venue.enabled площадки нет → да — подъём подписки; да → нет — запись, держится ли поток до
/// закрытия сделок (feed_hold.rs), и пересборка снимает или держит подписку. Ошибок нет.
pub fn on_setting(core: &Core, c: &Change) {
    if c.name != "venue.enabled" {
        return;
    }
    let Scope::Venue(id) = &c.scope else { return };
    let venue = VenueId(id.clone());
    let on = c.new.as_ref().and_then(Value::as_bool).unwrap_or(false);
    let was = c.old.as_ref().and_then(Value::as_bool).unwrap_or(false);
    let core = core.clone();
    match (was, on) {
        (false, true) => {
            tokio::spawn(async move {
                core.revive_feed(&venue, "Биржа включена").await;
            });
        }
        (true, false) => {
            tokio::spawn(async move { super::feed_hold::on_disabled(&core, &venue).await });
        }
        _ => {}
    }
}
