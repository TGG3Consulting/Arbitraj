//! Файл: notify/updates.rs
//! Что делает: принимает нажатия кнопок и сообщения Telegram длинным опросом getUpdates (у системы нет
//! публичного HTTPS-адреса для webhook): нажатие — в notify/press.rs, сообщение (команды /help, /menu …, ответ на
//! вопрос суммы) — в notify/tg_commands.rs (решения 156, 157). После подключения — getMe (имя бота для «/cmd@бот»)
//! и setMyCommands (подсказка команд по «/»).
//! offset: после каждого ответа — наибольший update_id + 1 (замечание 2 к getUpdates:
//! «recalculate offset after each server response»); обновление подтверждается следующим вызовом.
//! Кто вызывает: main.rs (запускает задачу run). Ожидание после сбоев — updates_idle.rs.
//! От чего зависит: notify/client.rs, notify/tg_buttons.rs, notify/press.rs, notify/tg_commands.rs, notify/tg_methods.rs,
//! notify/mirror_state.rs, core (команды — Core::command), settings, secrets, storage/journal.
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

use std::sync::Arc;

use tokio::sync::Notify;

use crate::secrets::Vault;
use crate::settings::live::Watch;
use crate::settings::read::At;
use crate::settings::store::Store;
use crate::storage::journal::{Journal, Level};

use crate::core::Core;

use super::client;
use super::mirror_state::Shared;
use super::press::Ctx;
use super::tg_commands::COMMANDS;
use super::updates_idle::retry_of;

/// Префиксы настроек, при изменении которых приём пробует снова.
pub const WATCH_PREFIXES: &[&str] = &["notify.", "http.timeout_ms"];

/// Приём нажатий кнопок Telegram.
pub struct Updates {
    pub http: reqwest::Client,
    pub settings: Store,
    pub vault: Vault,
    pub journal: Journal,
    /// Ядро — команды человека (Core::command) и данные меню (Core::control, Core::manual_records).
    pub core: Core,
    /// Память дубля «Ручного управления» (общая с notify/mirror.rs).
    pub state: Shared,
    /// Сигнал «секреты изменились» (разблокировка, смена токена): прервать опрос, сбросить offset
    /// (у другого бота свои update_id), начать заново. Подаёт тот, кто меняет секреты (api/secrets.rs).
    pub wake: Arc<Notify>,
}

/// Чем закончилось ожидание.
pub(super) enum Woke {
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
    // Имя бота (getMe) после подключения; None — узнать заново (после ожидания: могли смениться токен или чат).
    let mut bot: Option<String> = None;
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
        if bot.is_none() {
            match ready.tg.get_me().await {
                Ok(name) => {
                    if let Err(e) = ready.tg.set_my_commands(COMMANDS).await {
                        u.log(Level::Warn, format!("setMyCommands: {e}")).await;
                    }
                    bot = Some(name);
                }
                Err(e) => {
                    u.log(
                        Level::Error,
                        format!("Кнопки и команды Telegram: getMe: {e}"),
                    )
                    .await;
                    let retry = retry_of(&u, &e);
                    if !u.idle(&mut watch, &mut offset, retry).await {
                        return;
                    }
                    continue;
                }
            }
        }
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
                bot = None;
                continue;
            }
        };
        match polled {
            Ok(list) => {
                let ctx = Ctx {
                    tg: &ready.tg,
                    chat: &ready.chat,
                    bot: bot.as_deref().unwrap_or_default(),
                    core: &u.core,
                    journal: &u.journal,
                    state: &u.state,
                };
                for upd in list {
                    offset = Some(offset.map_or(upd.update_id + 1, |o| o.max(upd.update_id + 1)));
                    if let Some(q) = upd.callback_query {
                        ctx.handle(q).await;
                    }
                    if let Some(m) = upd.message {
                        ctx.message(m).await;
                    }
                }
            }
            Err(e) => {
                let retry = retry_of(&u, &e);
                u.log(Level::Error, format!("getUpdates: {e}")).await;
                bot = None;
                if !u.idle(&mut watch, &mut offset, retry).await {
                    return;
                }
            }
        }
    }
}
