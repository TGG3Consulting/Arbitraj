//! Файл: notify/updates_idle.rs
//! Что делает: ожидание приёма Telegram (продолжение updates.rs — размер файла): после ошибки — пауза retry_after из
//! ответа Telegram (ResponseParameters, https://core.telegram.org/bots/api#responseparameters) или
//! notify.updates_retry_ms; без паузы — до изменения настроек или сигнала wake (смена секретов: offset сбрасывается).
//! Кто вызывает: notify/updates.rs.
//! От чего зависит: settings, storage/journal, notify/tg_methods.rs.
//! Настройки: notify.updates_retry_ms.

use std::time::Duration;

use crate::settings::live::Watch;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::telegram::TgError;
use super::updates::{Updates, Woke};

impl Updates {
    /// Ждёт: изменения настроек, сигнала wake или (если задано) паузы retry_after от Telegram.
    /// Принимает: подписку, offset (сбрасывается по wake), паузу. Возвращает: false — система останавливается.
    pub(super) async fn idle(
        &self,
        watch: &mut Watch,
        offset: &mut Option<i64>,
        retry: Option<Duration>,
    ) -> bool {
        let woke = tokio::select! {
            c = watch.next() => if c.is_some() { Woke::Settings } else { Woke::Stop },
            _ = self.wake.notified() => Woke::Secrets,
            _ = sleep_or_never(retry) => Woke::Settings,
        };
        match woke {
            Woke::Settings => true,
            Woke::Secrets => {
                *offset = None;
                true
            }
            Woke::Stop => false,
        }
    }

    /// Запись в журнал от имени уведомлений. Ошибка журнала уже выведена в лог процесса.
    pub(super) async fn log(&self, level: Level, message: String) {
        let _ = self
            .journal
            .write(level, "notify", None, message, None)
            .await;
    }
}

/// Пауза после ошибки: retry_after из ответа Telegram, иначе notify.updates_retry_ms (не задана — None).
pub(super) fn retry_of(u: &Updates, e: &TgError) -> Option<Duration> {
    super::tg_methods::retry_after(e)
        .map(Duration::from_secs)
        .or_else(|| {
            u.settings
                .integer("notify.updates_retry_ms", At::default())
                .ok()
                .map(Duration::from_millis)
        })
}

/// Пауза, если она задана; иначе — никогда не заканчивается (ждём другие сигналы).
async fn sleep_or_never(d: Option<Duration>) {
    match d {
        Some(d) => tokio::time::sleep(d).await,
        None => std::future::pending().await,
    }
}
