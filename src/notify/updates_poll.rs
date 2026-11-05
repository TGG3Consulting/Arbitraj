//! Файл: notify/updates_poll.rs
//! Что делает: один длинный опрос getUpdates на один токен бота (решение 176; продолжение updates.rs) — для всех ролей
//! с этим токеном. После подключения — getMe (имя бота для «/cmd@бот») и setMyCommands: команды «Общего» — как раньше
//! (вся область бота), у чата «Управление» — своя короткая подсказка (BotCommandScopeChat). Каждое обновление: чат →
//! роль среди ролей опроса (roles::of_chat); чужой чат — отказ и запись в журнал; иначе — нажатие в notify/press.rs,
//! сообщение — в notify/tg_commands.rs.
//! offset: после каждого ответа — наибольший update_id + 1 (замечание к getUpdates: «recalculate offset after each
//! server response»), хранится по отпечатку токена (updates.rs::Offsets).
//! После ошибки — пауза: retry_after из ответа Telegram (ResponseParameters,
//! https://core.telegram.org/bots/api#responseparameters) сильнее notify.updates_retry_ms; не задана — опрос
//! останавливается до пересборки (изменение настроек или wake), как раньше.
//! Кто вызывает: notify/updates.rs.
//! От чего зависит: notify/client.rs, notify/tg_buttons.rs, notify/press.rs, notify/tg_commands.rs, tg_methods.rs,
//! notify/roles.rs, settings, storage/journal.
//! Настройки: notify.updates_timeout_s, notify.updates_retry_ms; ID чатов ролей; секреты токенов ролей.
//!
//! getUpdates: https://core.telegram.org/bots/api#getupdates; setMyCommands, BotCommandScopeChat:
//! https://core.telegram.org/bots/api#setmycommands, https://core.telegram.org/bots/api#botcommandscopechat

use std::time::Duration;

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::client;
use super::press::Ctx;
use super::roles::{self, Role};
use super::telegram::TgError;
use super::tg_buttons::Update;
use super::tg_commands::{COMMANDS, CONTROL_COMMANDS};
use super::updates::{Offsets, Updates};

/// Опрос одного токена. Принимает: приём, отпечаток токена, роли с этим токеном, общие offset.
/// Возвращает: только когда опрос остановлен (ошибка без паузы) — до пересборки. Ошибок нет: всё в журнал.
pub async fn run(u: &Updates, mark: u64, group: &[Role], offsets: &Offsets) {
    let Some(&first) = group.first() else {
        return;
    };
    let names: Vec<&str> = group.iter().map(|r| r.title()).collect();
    let names = names.join(", ");
    let mut bot: Option<String> = None;
    loop {
        let ready = match client::build_role(&u.http, &u.settings, &u.vault, first) {
            Ok(r) => r,
            Err(e) => {
                return u
                    .log(
                        Level::Warn,
                        format!("Кнопки чатов «{names}» не принимаются: {}", e.message),
                    )
                    .await;
            }
        };
        if bot.is_none() {
            match ready.tg.get_me().await {
                Ok(name) => {
                    commands(u, &ready.tg, group).await;
                    bot = Some(name);
                }
                Err(e) => {
                    u.log(Level::Error, format!("Кнопки чатов «{names}»: getMe: {e}"))
                        .await;
                    if !pause(u, &e).await {
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
                return u
                    .log(
                        Level::Warn,
                        format!("Кнопки Telegram не принимаются: {}", e.message),
                    )
                    .await;
            }
        };
        let offset = lock(offsets).get(&mark).copied();
        match ready.tg.get_updates(offset, timeout_s).await {
            Ok(list) => {
                for upd in list {
                    {
                        let mut o = lock(offsets);
                        let next = o
                            .get(&mark)
                            .map_or(upd.update_id + 1, |x| (*x).max(upd.update_id + 1));
                        o.insert(mark, next);
                    }
                    one(u, &ready.tg, bot.as_deref().unwrap_or_default(), group, upd).await;
                }
            }
            Err(e) => {
                u.log(Level::Error, format!("getUpdates (чаты «{names}»): {e}"))
                    .await;
                bot = None;
                if !pause(u, &e).await {
                    return;
                }
            }
        }
    }
}

/// Одно обновление: роль по чату, затем нажатие или сообщение. Ошибок нет.
async fn one(u: &Updates, tg: &super::telegram::Telegram, bot: &str, group: &[Role], upd: Update) {
    let chat = upd
        .callback_query
        .as_ref()
        .and_then(|q| q.message.as_ref().map(|m| m.chat.clone()))
        .or_else(|| upd.message.as_ref().map(|m| m.chat.clone()));
    let Some(chat) = chat else {
        if let Some(q) = &upd.callback_query {
            let _ = tg.answer_callback(&q.id, None).await;
        }
        return;
    };
    let Some(role) = roles::of_chat(&u.settings, &chat, group) else {
        if let Some(q) = &upd.callback_query {
            let _ = tg.answer_callback(&q.id, None).await;
        }
        return u
            .log(
                Level::Warn,
                format!("Обновление из чужого чата {} — игнор", chat.id),
            )
            .await;
    };
    let Some(setting) = roles::chat(&u.settings, role) else {
        return;
    };
    let ctx = Ctx {
        tg,
        chat: &setting,
        role,
        bot,
        core: &u.core,
        journal: &u.journal,
        settings: &u.settings,
        state: &u.state,
        ctl: &u.ctl,
    };
    if let Some(q) = upd.callback_query {
        ctx.handle(q).await;
    }
    if let Some(m) = upd.message {
        ctx.message(m).await;
    }
}

/// setMyCommands: «Общий» — список COMMANDS для всей области бота (как раньше); «Управление» — CONTROL_COMMANDS
/// только для его чата. Ошибка — в журнал. Ошибок не возвращает.
async fn commands(u: &Updates, tg: &super::telegram::Telegram, group: &[Role]) {
    if group.contains(&Role::General)
        && let Err(e) = tg.set_my_commands(COMMANDS, None).await
    {
        u.log(Level::Warn, format!("setMyCommands: {e}")).await;
    }
    if group.contains(&Role::Control)
        && let Some(c) = roles::chat(&u.settings, Role::Control)
        && let Err(e) = tg.set_my_commands(CONTROL_COMMANDS, Some(&c)).await
    {
        u.log(
            Level::Warn,
            format!("setMyCommands (чат «Управление»): {e}"),
        )
        .await;
    }
}

/// Пауза после ошибки: retry_after Telegram, иначе notify.updates_retry_ms. Возвращает: false — паузы нет (опрос
/// останавливается до пересборки). Ошибок нет.
async fn pause(u: &Updates, e: &TgError) -> bool {
    let d = super::tg_methods::retry_after(e)
        .map(Duration::from_secs)
        .or_else(|| {
            u.settings
                .integer("notify.updates_retry_ms", At::default())
                .ok()
                .map(Duration::from_millis)
        });
    match d {
        Some(d) => {
            tokio::time::sleep(d).await;
            true
        }
        None => false,
    }
}

/// Замок offset (отравленный — как есть).
fn lock(o: &Offsets) -> std::sync::MutexGuard<'_, std::collections::HashMap<u64, i64>> {
    o.lock().unwrap_or_else(|p| p.into_inner())
}
