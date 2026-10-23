//! Файл: notify/press.rs
//! Что делает: обрабатывает одно нажатие кнопки Telegram (CallbackQuery):
//! принимает только из настроенного чата notify.telegram_chat — чужие игнорируются и пишутся в журнал;
//! разбирает callback_data в команду ядру (ManualCommand) и кладёт её в канал команд ядра;
//! отвечает на каждое нажатие (answerCallbackQuery); после переданной команды правит сообщение —
//! убирает кнопки и дописывает результат, чтобы второй раз не нажать.
//! Кто вызывает: notify/updates.rs — на каждое Update с callback_query.
//! От чего зависит: notify/tg_buttons.rs, notify/callback.rs, core/types/control.rs, storage/journal.
//! Настройки: notify.telegram_chat (передаёт вызывающий).

use std::collections::HashSet;

use chrono::Utc;
use serde_json::json;
use tokio::sync::mpsc;

use crate::core::types::{CommandSource, ManualCommand};
use crate::storage::journal::{Journal, Level};

use super::callback;
use super::telegram::Telegram;
use super::tg_buttons::{CallbackQuery, Chat};

/// Канал команд человека ядру. Стык: тип канала утверждает техлид (запрос в Issue #10).
pub type CommandTx = mpsc::Sender<ManualCommand>;

/// Всё, что нужно для обработки нажатий. Живёт, пока работает приём (notify/updates.rs).
pub struct Press<'a> {
    pub tg: &'a Telegram,
    /// Значение notify.telegram_chat: числовой id чата или @username.
    pub chat: &'a str,
    pub commands: &'a CommandTx,
    pub journal: &'a Journal,
    /// Сообщения (чат, id), по которым команда уже передана, — защита от второго нажатия,
    /// пока правка сообщения не дошла. Только в памяти; растёт на одно значение на каждое решение человека.
    pub handled: &'a mut HashSet<(i64, i64)>,
}

/// Из настроенного ли чата нажатие.
/// Принимает: notify.telegram_chat и чат сообщения с кнопкой.
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

impl Press<'_> {
    /// Обрабатывает нажатие. Ошибок не возвращает: всё пишется в журнал, нажавший получает ответ.
    pub async fn handle(&mut self, q: CallbackQuery) {
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
        let parsed = q.data.as_deref().and_then(callback::decode);
        let Some((button, object, action)) =
            parsed.and_then(|(b, o)| callback::to_action(b, o).map(|a| (b, o, a)))
        else {
            self.answer(&q.id, Some("Неизвестная кнопка")).await;
            self.log(
                Level::Warn,
                format!("Неизвестная кнопка {:?} от {who}", q.data),
            )
            .await;
            return;
        };
        let title = callback::title(button).unwrap_or(button);
        let key = (msg.chat.id, msg.message_id);
        if self.handled.contains(&key) {
            self.answer(&q.id, Some("Уже обработано")).await;
            return;
        }
        let cmd = ManualCommand {
            action,
            source: CommandSource::Telegram {
                chat: msg.chat.id.to_string(),
            },
            at: Utc::now(),
        };
        let result = match self.commands.try_send(cmd) {
            Ok(()) => {
                self.handled.insert(key);
                Ok(format!("«{title}»: команда передана ядру"))
            }
            Err(mpsc::error::TrySendError::Full(_)) => Err(format!(
                "«{title}»: очередь команд ядра заполнена — команда не передана, нажмите ещё раз"
            )),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(format!(
                "«{title}»: ядро не принимает команды — команда не передана"
            )),
        };
        let (level, text) = match &result {
            Ok(t) => (Level::Info, t.clone()),
            Err(t) => (Level::Error, t.clone()),
        };
        self.answer(&q.id, Some(&text)).await;
        let data = json!({ "button": button, "object": object, "from": who, "chat": msg.chat.id, "message_id": msg.message_id });
        let _ = self
            .journal
            .write(
                level,
                "notify",
                None,
                format!("{text} (нажал {who})"),
                Some(data),
            )
            .await;
        if result.is_ok() {
            let at = Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
            let body = format!(
                "{}\n\n{text}. Нажал {who}, {at}.",
                msg.text.as_deref().unwrap_or_default()
            );
            if let Err(e) = self
                .tg
                .edit_text_drop_keyboard(msg.chat.id, msg.message_id, body.trim_start())
                .await
            {
                self.log(
                    Level::Error,
                    format!("Не удалось убрать кнопки у сообщения: {e}"),
                )
                .await;
            }
        }
    }

    /// answerCallbackQuery; ошибка — в журнал. Принимает: id нажатия, текст ответа.
    async fn answer(&self, id: &str, text: Option<&str>) {
        if let Err(e) = self.tg.answer_callback(id, text).await {
            self.log(Level::Warn, format!("answerCallbackQuery: {e}"))
                .await;
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
