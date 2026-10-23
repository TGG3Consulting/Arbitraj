//! Файл: notify/updates.rs
//! Что делает: принимает нажатия кнопок Telegram длинным опросом getUpdates (у системы нет
//! публичного HTTPS-адреса для webhook) и передаёт каждое нажатие в notify/press.rs.
//! offset: после каждого ответа — наибольший update_id + 1 (замечание 2 к getUpdates:
//! «recalculate offset after each server response»); обновление подтверждается следующим вызовом.
//! Кто вызывает: main.rs (запускает задачу run).
//! От чего зависит: notify/client.rs, notify/tg_buttons.rs, notify/press.rs, settings, secrets, storage/journal.
//! Настройки: notify.updates_timeout_s (параметр timeout getUpdates), notify.updates_retry_ms, notify.telegram_chat,
//! notify.telegram_api_url, http.timeout_ms; секрет telegram.bot_token.
//!
//! Нет настройки, токена или связи — приём не работает, причина пишется в журнал, задача ждёт
//! изменения настроек notify.* / http.timeout_ms или сигнала wake (разблокировка или смена секретов).
//! После ошибки сети или Telegram — пауза notify.updates_retry_ms и повтор (не задана — ждём изменения
//! настроек или wake); retry_after из ответа Telegram (ResponseParameters,
//! https://core.telegram.org/bots/api#responseparameters) сильнее: ждём столько, сколько сказал Telegram.
//!
//! getUpdates: https://core.telegram.org/bots/api#getupdates

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::secrets::Vault;
use crate::settings::live::Watch;
use crate::settings::read::At;
use crate::settings::store::Store;
use crate::storage::journal::{Journal, Level};

use super::client;
use super::press::{CommandTx, Press};
use super::telegram::TgError;

/// Префиксы настроек, при изменении которых приём пробует снова.
pub const WATCH_PREFIXES: &[&str] = &["notify.", "http.timeout_ms"];

/// Приём нажатий кнопок Telegram.
pub struct Updates {
    pub http: reqwest::Client,
    pub settings: Store,
    pub vault: Vault,
    pub journal: Journal,
    /// Канал команд человека ядру.
    pub commands: CommandTx,
    /// Сигнал «секреты изменились» (разблокировка, смена токена): прервать опрос, сбросить offset
    /// (у другого бота свои update_id), начать заново. Подаёт тот, кто меняет секреты (api/secrets.rs).
    pub wake: Arc<Notify>,
}

/// Чем закончилось ожидание.
enum Woke {
    /// Изменились настройки — пробуем снова.
    Settings,
    /// Сигнал wake — сбросить offset и пробовать снова.
    Secrets,
    /// Рассылка настроек закрыта — система останавливается.
    Stop,
}

/// Работает, пока система запущена. Принимает: приём и подписку на настройки (префиксы WATCH_PREFIXES).
/// Ошибок не возвращает: всё пишется в журнал.
pub async fn run(u: Updates, mut watch: Watch) {
    let mut offset: Option<i64> = None;
    let mut handled = HashSet::new();
    loop {
        let ready = match client::build(&u.http, &u.settings, &u.vault) {
            Ok(r) => r,
            Err(e) => {
                u.log(
                    Level::Warn,
                    format!("Кнопки Telegram не принимаются: {}", e.message),
                )
                .await;
                if !u.idle(&mut watch, &mut offset, None).await {
                    return;
                }
                continue;
            }
        };
        let timeout_s = match u
            .settings
            .integer("notify.updates_timeout_s", At::default())
        {
            Ok(t) => t,
            Err(e) => {
                u.log(
                    Level::Warn,
                    format!("Кнопки Telegram не принимаются: {}", e.message),
                )
                .await;
                if !u.idle(&mut watch, &mut offset, None).await {
                    return;
                }
                continue;
            }
        };
        let polled = tokio::select! {
            r = ready.tg.get_updates(offset, timeout_s) => r,
            _ = u.wake.notified() => {
                offset = None;
                continue;
            }
        };
        match polled {
            Ok(list) => {
                let mut press = Press {
                    tg: &ready.tg,
                    chat: &ready.chat,
                    commands: &u.commands,
                    journal: &u.journal,
                    handled: &mut handled,
                };
                for upd in list {
                    offset = Some(offset.map_or(upd.update_id + 1, |o| o.max(upd.update_id + 1)));
                    if let Some(q) = upd.callback_query {
                        press.handle(q).await;
                    }
                }
            }
            Err(e) => {
                let retry = match &e {
                    TgError::Api {
                        retry_after: Some(s),
                        ..
                    } if *s > 0 => Some(Duration::from_secs(s.unsigned_abs())),
                    _ => u
                        .settings
                        .integer("notify.updates_retry_ms", At::default())
                        .ok()
                        .map(Duration::from_millis),
                };
                u.log(Level::Error, format!("getUpdates: {e}")).await;
                if !u.idle(&mut watch, &mut offset, retry).await {
                    return;
                }
            }
        }
    }
}

impl Updates {
    /// Ждёт: изменения настроек, сигнала wake или (если задано) паузы retry_after от Telegram.
    /// Принимает: подписку, offset (сбрасывается по wake), паузу. Возвращает: false — система останавливается.
    async fn idle(
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
    async fn log(&self, level: Level, message: String) {
        let _ = self
            .journal
            .write(level, "notify", None, message, None)
            .await;
    }
}

/// Пауза, если она задана; иначе — никогда не заканчивается (ждём другие сигналы).
async fn sleep_or_never(d: Option<Duration>) {
    match d {
        Some(d) => tokio::time::sleep(d).await,
        None => std::future::pending().await,
    }
}
