//! Файл: notify/press.rs
//! Что делает: одно нажатие кнопки Telegram (CallbackQuery): принимает только из настроенного чата notify.telegram_chat
//! (чужие игнорируются и пишутся в журнал), разбирает callback_data и передаёт нужному обработчику:
//! «<код>:<ключ записи>» — кнопка записи «Ручного управления» (press_record.rs, решение 156);
//! «y:<n>» / «n:<n>» — «Да, …» / «Отмена» подтверждения (press_confirm.rs); «m:<экран>» — экран меню (menu_press.rs,
//! решение 157); «c:<что>» — кнопка плашки режима в меню (menu_press.rs). На каждое нажатие — answerCallbackQuery.
//! Кто вызывает: notify/updates.rs — на каждое Update с callback_query.
//! От чего зависит: notify/callback.rs (decode), press_record.rs, press_confirm.rs, menu_press.rs, tg_buttons.rs, core.
//! Настройки: notify.telegram_chat (передаёт вызывающий).

use crate::core::Core;
use crate::storage::journal::{Journal, Level};

use super::callback;
use super::mirror_state::Shared;
use super::telegram::Telegram;
use super::tg_buttons::{CallbackQuery, Chat};

/// Всё, что нужно обработчикам нажатий и сообщений. Живёт, пока работает приём (notify/updates.rs).
pub struct Ctx<'a> {
    pub tg: &'a Telegram,
    /// Значение notify.telegram_chat: числовой id чата или @username.
    pub chat: &'a str,
    /// Имя бота (getMe) — для команд вида /cmd@имя_бота в группах.
    pub bot: &'a str,
    pub core: &'a Core,
    pub journal: &'a Journal,
    /// Память дубля «Ручного управления» (общая с notify/mirror.rs).
    pub state: &'a Shared,
}

/// Из настроенного ли чата нажатие или сообщение.
/// Принимает: notify.telegram_chat и чат сообщения.
/// Возвращает: true — значение настройки равно id чата, или это «@username» и он совпадает с username чата
/// (sendMessage принимает оба вида chat_id, https://core.telegram.org/bots/api#sendmessage).
pub fn chat_matches(setting: &str, chat: &Chat) -> bool {
    if setting == chat.id.to_string() {
        return true;
    }
    match (setting.strip_prefix('@'), chat.username.as_deref()) {
        (Some(want), Some(have)) => want == have,
        _ => false,
    }
}

impl Ctx<'_> {
    /// Обрабатывает нажатие. Ошибок не возвращает: всё пишется в журнал, нажавший получает ответ.
    pub async fn handle(&self, q: CallbackQuery) {
        let who = q.from.describe();
        let Some(msg) = q.message.as_ref() else {
            self.answer(&q.id, None).await;
            self.log(
                Level::Warn,
                format!("Нажатие без сообщения бота (inline-режим) от {who} — игнор"),
            )
            .await;
            return;
        };
        if !chat_matches(self.chat, &msg.chat) {
            self.answer(&q.id, None).await;
            self.log(
                Level::Warn,
                format!("Нажатие из чужого чата {} от {who} — игнор", msg.chat.id),
            )
            .await;
            return;
        }
        let Some((code, object)) = q.data.as_deref().and_then(callback::decode) else {
            self.answer(&q.id, Some("Неизвестная кнопка")).await;
            self.log(
                Level::Warn,
                format!("Неизвестная кнопка {:?} от {who}", q.data),
            )
            .await;
            return;
        };
        match code {
            "y" | "n" => match object.parse::<u64>() {
                Ok(n) => self.confirm(&q, msg, n, code == "y").await,
                Err(_) => self.answer(&q.id, Some("Неизвестная кнопка")).await,
            },
            "m" => self.menu_screen(&q, msg, object).await,
            "c" => self.menu_control(&q, msg, object).await,
            _ => self.record_button(&q, msg, code, object).await,
        }
    }

    /// answerCallbackQuery; ошибка — в журнал. Принимает: id нажатия, текст ответа.
    pub async fn answer(&self, id: &str, text: Option<&str>) {
        if let Err(e) = self.tg.answer_callback(id, text).await {
            self.log(Level::Warn, format!("answerCallbackQuery: {e}"))
                .await;
        }
    }

    /// Запись в журнал от имени уведомлений. Ошибка журнала уже выведена в лог процесса.
    pub async fn log(&self, level: Level, message: String) {
        let _ = self
            .journal
            .write(level, "notify", None, message, None)
            .await;
    }
}
